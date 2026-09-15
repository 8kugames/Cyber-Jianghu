// ============================================================================
// DirectLlmClient HTTP 传输层（OpenAI 兼容 /chat/completions）
// ============================================================================
//
// 非流式 send_request / 伪流式 send_request_via_stream / 真流式 send_streaming_request：
// 共享 breaker 守门、网络错误立即重试一次、token 记账与 prefix-cache 诊断。

use super::super::openai_types::{OpenAIRequest, OpenAIResponse};
use super::super::token_tracking::record_token_usage;
use super::DirectLlmClient;
use super::{estimate_prompt_tokens, track_system_hash, utf8_safe_end};
use anyhow::{Context, Result};
use std::sync::atomic::Ordering;
use tracing::{debug, error, info};

impl DirectLlmClient {
    /// 发送 OpenAI 兼容 API 请求（公共 HTTP 逻辑）
    pub(super) async fn send_request(&self, request: &OpenAIRequest) -> Result<OpenAIResponse> {
        // 共享 breaker 守门：模型在冷却期直接拒绝
        self.check_breaker()?;

        // prefer_stream=false: 优先走非流式，失败后降级流式
        // 非流式路径自带 400→stream 兜底（line 616），确保只支持 streaming 的模型正常工作
        if self.config.prefer_stream.load(Ordering::Relaxed) {
            match self.send_request_via_stream(request).await {
                Ok(r) => return Ok(r),
                Err(e) => {
                    tracing::warn!("[地魂] 流式请求失败，降级非流式: {}", e);
                }
            }
        }

        let client = self.build_http_client()?;
        let base_url = self.config.get_base_url()?;
        let base_url = base_url.trim_end_matches('/');
        let url = if base_url.contains("/chat/completions") {
            base_url.to_string()
        } else {
            format!("{}/chat/completions", base_url)
        };

        debug!("Calling OpenAI-compatible API: {}", url);
        debug!("Request model: {}", request.model);
        // 地魂诊断：确认 tools 字段是否在请求中
        if request.tools.is_some() {
            info!(
                "[地魂] 发送请求: url={}, model={}, tools={}, tool_choice={}, stream={:?}, prefer_stream={}",
                url,
                request.model,
                request.tools.as_ref().map(|t| t.len()).unwrap_or(0),
                request
                    .tool_choice
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "None".to_string()),
                request.stream,
                self.config.prefer_stream.load(Ordering::Relaxed),
            );
        }

        let mut request_builder = client.post(&url).header("Content-Type", "application/json");

        if let Some(ref api_key) = self.config.api_key {
            request_builder =
                request_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        // 网络错误时立即重试一次（无等待，避免积压）
        let response = match request_builder.try_clone() {
            Some(rb1) => match rb1.json(&request).send().await {
                Ok(r) => r,
                Err(e) if e.is_connect() || e.is_timeout() || e.is_request() => {
                    tracing::warn!("LLM 请求发送失败（网络错误），立即重试一次: {}", e);
                    request_builder
                        .json(&request)
                        .send()
                        .await
                        .context("LLM API request failed after 1 retry")?
                }
                Err(e) => return Err(e).context("Failed to send request to LLM API"),
            },
            None => {
                // 无法 clone，直接请求
                request_builder
                    .json(&request)
                    .send()
                    .await
                    .context("Failed to send request to LLM API")?
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response
                .bytes()
                .await
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_else(|_| "Unable to read error body".to_string());
            error!("LLM API error {}: {}", status, error_body);

            // 400 + "stream"：模型强制要求流式，自动用流式重试
            if status.as_u16() == 400 && error_body.contains("stream") {
                info!("模型要求流式调用，自动切换到 streaming 重试，后续将直接走流式");
                self.config.prefer_stream.store(true, Ordering::Relaxed);
                return self.send_request_via_stream(request).await;
            }

            super::super::token_tracking::record_failure(
                &self.config.provider,
                &self.config.get_model_with_default(),
            );
            anyhow::bail!("LLM API error {}: {}", status, error_body);
        }

        // DEBUG: 工具调用时打印原始响应 body 的 tool_calls 部分
        let raw_bytes = response
            .bytes()
            .await
            .context("Failed to read response body")?;
        let raw_body = String::from_utf8(raw_bytes.to_vec())
            .context("LLM response body is not valid UTF-8")?;

        if raw_body.trim().is_empty() {
            super::super::token_tracking::record_failure(
                &self.config.provider,
                &self.config.get_model_with_default(),
            );
            anyhow::bail!("LLM returned empty response body");
        }

        debug!(
            "[地魂] raw_body 前200字符: {}",
            raw_body.chars().take(2000).collect::<String>()
        );
        if request.tools.is_some() {
            let tool_calls_preview = if let Some(tc_start) = raw_body.find("\"tool_calls\"") {
                let end = utf8_safe_end(&raw_body, tc_start + 3000);
                &raw_body[tc_start..end]
            } else {
                "tool_calls field NOT FOUND in response"
            };
            debug!(
                "[地魂] 原始 API 响应 (tool_calls 片段): {}",
                tool_calls_preview
            );
        }
        let response_data: OpenAIResponse = serde_json::from_str(&raw_body).map_err(|e| {
            super::super::token_tracking::record_failure(
                &self.config.provider,
                &self.config.get_model_with_default(),
            );
            tracing::warn!(
                "LLM response JSON parse failed: provider={}, raw_body_len={}",
                self.config.provider.as_str(),
                raw_body.len()
            );
            anyhow::anyhow!("Failed to parse LLM response: {}", e)
        })?;

        let model = self.config.get_model_with_default();
        if let Some(ref actual_model) = response_data.model
            && actual_model != &model
        {
            info!(
                "[llm] model fallback: requested={}, actual={}",
                model, actual_model
            );
        }
        let system_hash = Self::extract_system_hash_from_request(request);
        if let Some(ref usage) = response_data.usage {
            let cache_hit = usage.cache_hit_tokens().unwrap_or(0);
            record_token_usage(
                &self.config.provider,
                &model,
                usage.prompt_tokens,
                usage.completion_tokens,
                cache_hit,
                system_hash,
            );
            self.emit_cache_diagnostics(system_hash, usage.prompt_tokens, cache_hit, &model);
            debug!(
                "Token usage: provider={}, model={}, prompt={}, completion={}, cache_hit={}",
                self.config.provider.as_str(),
                model,
                usage.prompt_tokens,
                usage.completion_tokens,
                cache_hit,
            );
        } else {
            // API 未返回 usage，按字符长度估算
            let est_pt = estimate_prompt_tokens(request);
            let est_ct = response_data
                .choices
                .first()
                .and_then(|c| c.message.content.as_ref())
                .map(|s| (s.len() as u64 / 3).max(1))
                .unwrap_or(0);
            record_token_usage(
                &self.config.provider,
                &model,
                est_pt,
                est_ct,
                0,
                system_hash,
            );
            debug!(
                "Token usage (estimated): provider={}, model={}, prompt~{}, completion~{}",
                self.config.provider.as_str(),
                model,
                est_pt,
                est_ct
            );
        }

        Ok(response_data)
    }

    /// 从 request 中提取 system 字符串（按 OpenAI 约定 messages[0] 为 system）
    /// 若 messages[0] 不是 system role 或 content 为 None, 返回空字符串 (hash 仍可计算, 区别于 `[0u8;32]`)
    pub(super) fn extract_system_from_request(request: &OpenAIRequest) -> String {
        request
            .messages
            .first()
            .filter(|m| m.role == "system")
            .and_then(|m| m.content.clone())
            .unwrap_or_default()
    }

    pub(super) fn extract_system_hash_from_request(request: &OpenAIRequest) -> [u8; 32] {
        let system = Self::extract_system_from_request(request);
        crate::soul::actor::compute_system_hash(&system)
    }

    pub(super) fn emit_cache_diagnostics(
        &self,
        system_hash: [u8; 32],
        prompt_tokens: u64,
        cache_hit: u64,
        model: &str,
    ) {
        let hash_hex = hex::encode(system_hash);

        if let Ok(mut guard) = self.known_system_hashes.lock()
            && track_system_hash(&mut guard, system_hash)
        {
            tracing::warn!(
                target: "cache_diagnostics",
                new_hash = %hash_hex,
                model = %model,
                "system_hash_new — 出现未见过的 system prompt 前缀，provider cache 失效"
            );
        }

        if crate::config::env_or("CYBER_JIANGHU_CACHE_DIAGNOSTICS_ENABLED", true) {
            let cache_ratio = if prompt_tokens > 0 {
                cache_hit as f64 / prompt_tokens as f64
            } else {
                0.0
            };
            tracing::info!(
                target: "cache_diagnostics",
                system_hash = %hash_hex,
                prompt_tokens = prompt_tokens,
                cache_hit_tokens = cache_hit,
                cache_hit_rate = format!("{:.1}%", cache_ratio * 100.0),
                model = %model,
                "prefix_cache_tick"
            );
        }
    }

    /// 流式降级：用 streaming 收集完整响应，组装为 OpenAIResponse
    ///
    /// 当 send_request 遇到 "only support stream mode" 错误时调用此方法。
    /// 复用 send_streaming_request 建立 SSE 连接，收集全部 Delta 后拼装响应。
    pub(super) async fn send_request_via_stream(
        &self,
        request: &OpenAIRequest,
    ) -> Result<OpenAIResponse> {
        use super::super::streaming::StreamAccumulator;
        use futures_util::StreamExt;

        info!("[地魂] send_request_via_stream 入口（流式路径）");
        let system_hash = Self::extract_system_hash_from_request(request);
        let mut stream = self.send_streaming_request(request).await?;
        let mut acc = StreamAccumulator::new();
        let mut transport_error: Option<anyhow::Error> = None;

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(c) => acc.push(c),
                Err(e) => {
                    tracing::warn!("SSE 流中断: {}", e);
                    transport_error = Some(e);
                    break;
                }
            }
        }

        if let Some(e) = transport_error {
            return Err(e);
        }

        let json_complete = acc.is_json_complete();
        if acc.is_truncated() && !json_complete {
            let content_len = acc.content().len();
            tracing::warn!(
                "[地魂] 流式截断: finish_reason=length, content_len={}, JSON不完整, 委托重试机制",
                content_len,
            );
            return Err(anyhow::anyhow!(
                "EOF while parsing a value at line 1 column 0: response truncated by max_tokens (content_len={}, via stream)",
                content_len,
            ));
        }

        let stats = acc.token_stats();
        let pt = stats.prompt_tokens;
        let ct = stats.completion_tokens;
        let has_real = stats.has_real_usage;
        let cache_hit = stats.cache_hit_tokens.unwrap_or(0);
        let (content, tool_calls, reasoning_content) = acc.into_parts();

        // 诊断：检测 content 中的 UTF-8 mojibake（Latin-1 双重编码）
        if content.contains("Ã") || content.contains("Â") {
            let mojibake_positions: Vec<usize> = content
                .match_indices("Ã")
                .chain(content.match_indices("Â"))
                .take(5)
                .map(|(i, _)| i)
                .collect();
            tracing::warn!(
                "[地魂] UTF-8 mojibake detected in stream content! positions={:?}, snippet={:?}",
                mojibake_positions,
                &content[mojibake_positions.first().copied().unwrap_or(0)
                    ..content
                        .len()
                        .min(mojibake_positions.first().copied().unwrap_or(0) + 50)]
            );
        }

        // tool_calls 存在时不算空响应
        let has_tool_calls = !tool_calls.is_empty();
        let has_reasoning = !reasoning_content.trim().is_empty();

        // 空内容检测：SSE 流正常完成但 delta content 为空（content filtering 等）
        // 当 LLM 返回 tool_calls 或 reasoning_content 时，不应视为空响应
        if content.trim().is_empty() && !has_tool_calls && !has_reasoning {
            tracing::warn!(
                "[地魂] 空响应诊断: has_tool_calls={}, tool_calls_count={}, content_len={}, has_reasoning={}, reasoning_len={}, has_real_usage={}, pt={}, ct={}",
                has_tool_calls,
                tool_calls.len(),
                content.len(),
                has_reasoning,
                reasoning_content.len(),
                has_real,
                pt,
                ct,
            );
            if pt > 0 {
                record_token_usage(
                    &self.config.provider,
                    &self.config.get_model_with_default(),
                    pt,
                    0,
                    0,
                    system_hash,
                );
            }
            anyhow::bail!(
                "LLM API error: response content is empty (streaming, provider={}, model={}, prompt_tokens={}, completion_tokens={})",
                self.config.provider.as_str(),
                self.config.get_model_with_default(),
                pt,
                ct
            );
        }

        if has_tool_calls {
            let call_names: Vec<&str> = tool_calls
                .iter()
                .map(|tc| tc.function.name.as_str())
                .collect();
            debug!(
                "[地魂] 流式 tool_calls 累积完成: {} calls, names={:?}",
                tool_calls.len(),
                call_names
            );
        }

        // 记录流式 token 用量
        if pt > 0 || ct > 0 {
            // 当服务端未返回 usage（如 MiniMax），DoneEstimation 只估算了 completion，
            // pt=0 但 ct>0；需要根据请求内容补算 prompt。
            let final_pt = if pt == 0 && !has_real {
                estimate_prompt_tokens(request)
            } else {
                pt
            };
            record_token_usage(
                &self.config.provider,
                &self.config.get_model_with_default(),
                final_pt,
                ct,
                cache_hit,
                system_hash,
            );
            self.emit_cache_diagnostics(
                system_hash,
                final_pt,
                cache_hit,
                &self.config.get_model_with_default(),
            );
            debug!(
                "Stream token usage: provider={}, model={}, prompt={}, completion={}, cache_hit={}, real_usage={}",
                self.config.provider.as_str(),
                self.config.get_model_with_default(),
                final_pt,
                ct,
                cache_hit,
                has_real
            );
        } else if !has_real {
            // pt==0 且 ct==0 且无 usage（空响应降级），按请求内容全量估算
            let est_pt = estimate_prompt_tokens(request);
            let est_ct = (content.len() as u64 / 3).max(1);
            record_token_usage(
                &self.config.provider,
                &self.config.get_model_with_default(),
                est_pt,
                est_ct,
                0,
                system_hash,
            );
            debug!(
                "Stream token usage (estimated fallback): provider={}, model={}, prompt~{}, completion~{}",
                self.config.provider.as_str(),
                self.config.get_model_with_default(),
                est_pt,
                est_ct
            );
        }

        // 组装为 OpenAIResponse 格式（与 send_request 返回一致）
        let rc = if !reasoning_content.trim().is_empty() {
            Some(reasoning_content.clone())
        } else {
            None
        };
        Ok(OpenAIResponse {
            choices: vec![super::super::openai_types::OpenAIChoice {
                message: super::super::openai_types::ChatMessage {
                    role: "assistant".to_string(),
                    content: if !content.trim().is_empty() {
                        Some(content)
                    } else if !reasoning_content.trim().is_empty() {
                        tracing::info!(
                            "[地魂] content 为空但 reasoning_content 存在 ({} chars)，使用 reasoning 作为响应",
                            reasoning_content.len()
                        );
                        Some(reasoning_content)
                    } else {
                        None
                    },
                    tool_calls: if has_tool_calls {
                        Some(tool_calls)
                    } else {
                        None
                    },
                    tool_call_id: None,
                    name: None,
                    reasoning_content: rc,
                },
            }],
            usage: None,
            model: None,
        })
    }

    /// 发送流式请求到 OpenAI 兼容 API
    ///
    /// 返回 SSE 流，每个 chunk 为 StreamChunk::Delta 或 StreamChunk::Done
    pub(super) async fn send_streaming_request(
        &self,
        request: &OpenAIRequest,
    ) -> Result<super::super::streaming::LlmStream> {
        // 共享 breaker 守门：模型在冷却期直接拒绝
        self.check_breaker()?;

        let client = self.build_http_client()?;
        let base_url = self.config.get_base_url()?;
        let url = format!("{}/chat/completions", base_url);

        let mut request_builder = client.post(&url).header("Content-Type", "application/json");
        if let Some(ref api_key) = self.config.api_key {
            request_builder =
                request_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        // 设置 stream: true 和 stream_options: {"include_usage": true}
        // 这使得服务端在流式响应的最后一块返回 usage 数据
        let mut stream_request = request.clone();
        stream_request.stream = Some(true);
        stream_request.stream_options = Some(serde_json::json!({"include_usage": true}));

        // 网络错误时立即重试一次（无等待，避免积压）
        let response = match request_builder.try_clone() {
            Some(rb1) => match rb1.json(&stream_request).send().await {
                Ok(r) => r,
                Err(e) if e.is_connect() || e.is_timeout() || e.is_request() => {
                    tracing::warn!(
                        "LLM streaming 请求发送失败（网络错误），立即重试一次: {}",
                        e
                    );
                    request_builder
                        .json(&stream_request)
                        .send()
                        .await
                        .context("LLM streaming API request failed after 1 retry")?
                }
                Err(e) => return Err(e).context("Failed to send request to LLM streaming API"),
            },
            None => {
                // 无法 clone，直接请求
                request_builder
                    .json(&stream_request)
                    .send()
                    .await
                    .context("Failed to send request to LLM streaming API")?
            }
        };

        let status = response.status();
        if !status.is_success() {
            let error_body = response
                .bytes()
                .await
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            super::super::token_tracking::record_failure(
                &self.config.provider,
                &self.config.get_model_with_default(),
            );
            anyhow::bail!("LLM streaming API error {}: {}", status, error_body);
        }

        debug!(
            "LLM streaming connection established: provider={}, model={}",
            self.config.provider.as_str(),
            self.config.get_model_with_default(),
        );

        Ok(super::super::streaming::parse_sse_stream(response))
    }
}
