//! 角色生成 prompt 构建与 schema 校验辅助（自 character_register.rs 外移）

use super::*;

/// Resolve dot-notation path in JSON value (e.g. "language_style.tone")
pub(super) fn resolve_path<'a>(
    value: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

/// Schema-driven validation error
#[derive(Debug)]
pub(super) struct FieldValidationError {
    path: String,
    message: String,
}

impl std::fmt::Display for FieldValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Validate JSON value against field specs
pub(super) fn validate_against_schema(
    value: &serde_json::Value,
    fields: &[FieldSpec],
) -> Result<(), Vec<FieldValidationError>> {
    let mut errors = Vec::new();

    for spec in fields {
        let field_val = resolve_path(value, &spec.path);

        match &spec.constraints {
            FieldConstraints::String {
                required,
                min_chars,
                max_chars,
                ..
            } => match field_val {
                None | Some(serde_json::Value::Null) => {
                    if *required {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: "required field missing".into(),
                        });
                    }
                }
                Some(serde_json::Value::String(s)) => {
                    let len = s.chars().count();
                    if len < *min_chars {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: format!("min {} chars, got {}", min_chars, len),
                        });
                    }
                    if len > *max_chars {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: format!("max {} chars, got {}", max_chars, len),
                        });
                    }
                }
                Some(other) => {
                    errors.push(FieldValidationError {
                        path: spec.path.clone(),
                        message: format!("expected string, got {}", other),
                    });
                }
            },
            FieldConstraints::Integer { required, min, max } => match field_val {
                None | Some(serde_json::Value::Null) => {
                    if *required {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: "required field missing".into(),
                        });
                    }
                }
                Some(serde_json::Value::Number(n)) => {
                    if let Some(n) = n.as_u64() {
                        let n = n as u32;
                        if n < *min || n > *max {
                            errors.push(FieldValidationError {
                                path: spec.path.clone(),
                                message: format!("must be {}-{}, got {}", min, max, n),
                            });
                        }
                    } else {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: "expected integer".into(),
                        });
                    }
                }
                Some(other) => {
                    errors.push(FieldValidationError {
                        path: spec.path.clone(),
                        message: format!("expected integer, got {}", other),
                    });
                }
            },
            FieldConstraints::Enum { required, .. } => match field_val {
                None | Some(serde_json::Value::Null) => {
                    if *required {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: "required field missing".into(),
                        });
                    }
                }
                Some(serde_json::Value::String(_)) => {
                    // options 仅作示例，不强制校验
                }
                Some(other) => {
                    errors.push(FieldValidationError {
                        path: spec.path.clone(),
                        message: format!("expected string, got {}", other),
                    });
                }
            },
            FieldConstraints::EnumArray {
                required,
                min_count,
                max_count,
                ..
            } => match field_val {
                None | Some(serde_json::Value::Null) => {
                    if *required {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: "required field missing".into(),
                        });
                    }
                }
                Some(serde_json::Value::String(_)) => {
                    // options 仅作示例，不强制校验
                }
                Some(serde_json::Value::Array(arr)) => {
                    if arr.len() < *min_count || arr.len() > *max_count {
                        errors.push(FieldValidationError {
                            path: spec.path.clone(),
                            message: format!(
                                "need {}-{} items, got {}",
                                min_count,
                                max_count,
                                arr.len()
                            ),
                        });
                    }
                    // options 仅作示例，不强制校验元素值
                }
                Some(other) => {
                    errors.push(FieldValidationError {
                        path: spec.path.clone(),
                        message: format!("expected array, got {}", other),
                    });
                }
            },
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Resolve template variables in prompt_text from field constraints
pub(super) fn resolve_prompt_template(
    template: &str,
    spec: &FieldSpec,
    extra_vars: &std::collections::HashMap<String, String>,
) -> String {
    let mut result = template.to_string();
    match &spec.constraints {
        FieldConstraints::String {
            min_chars,
            max_chars,
            ..
        } => {
            result = result.replace("{min_chars}", &min_chars.to_string());
            result = result.replace("{max_chars}", &max_chars.to_string());
        }
        FieldConstraints::Integer { min, max, .. } => {
            result = result.replace("{min}", &min.to_string());
            result = result.replace("{max}", &max.to_string());
        }
        FieldConstraints::Enum { options, .. } => {
            result = result.replace("{options}", &options.join("\u{3001}"));
        }
        FieldConstraints::EnumArray {
            options,
            min_count,
            max_count,
            ..
        } => {
            result = result.replace("{options}", &options.join("\u{3001}"));
            result = result.replace("{min_count}", &min_count.to_string());
            result = result.replace("{max_count}", &max_count.to_string());
        }
    }
    for (k, v) in extra_vars {
        result = result.replace(&format!("{{{}}}", k), v);
    }
    result
}

/// Generate prompt field line from a single field spec
pub(super) fn generate_field_line(
    spec: &FieldSpec,
    extra_vars: &std::collections::HashMap<String, String>,
) -> String {
    let field_name = spec.path.split('.').next_back().unwrap_or(&spec.path);

    // Check for prompt_text override
    let prompt_text = match &spec.constraints {
        FieldConstraints::String {
            prompt_text: Some(txt),
            ..
        }
        | FieldConstraints::Enum {
            prompt_text: Some(txt),
            ..
        } => Some(txt.clone()),
        _ => None,
    };

    if let Some(txt) = prompt_text {
        let resolved = resolve_prompt_template(&txt, spec, extra_vars);
        return format!("- {}: {}", field_name, resolved);
    }

    // Auto-generate from constraints
    match &spec.constraints {
        FieldConstraints::String {
            max_chars,
            min_chars,
            ..
        } => {
            if *min_chars > 0 {
                format!("- {}: {}-{} chars", field_name, min_chars, max_chars)
            } else if *max_chars > 0 {
                format!("- {}: max {} chars", field_name, max_chars)
            } else {
                format!("- {}: string", field_name)
            }
        }
        FieldConstraints::Integer { min, max, .. } => {
            format!("- {}: {}-{} (integer)", field_name, min, max)
        }
        FieldConstraints::Enum { options, .. } => {
            format!(
                "- {}: pick 1 from: {}",
                field_name,
                options.join("\u{3001}")
            )
        }
        FieldConstraints::EnumArray {
            options,
            min_count,
            max_count,
            extra_prompt,
            ..
        } => {
            let base = format!(
                "- {}: pick {}-{} from: {}",
                field_name,
                min_count,
                max_count,
                options.join("\u{3001}")
            );
            if let Some(extra) = extra_prompt {
                format!("{}, {}", base, extra)
            } else {
                base
            }
        }
    }
}

/// Build full character generation prompt from schema
pub(super) fn generate_character_prompt(
    cg: &CharacterGenerationConfig,
    extra_vars: &std::collections::HashMap<String, String>,
) -> String {
    let mut top_level = Vec::new();
    let mut groups: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();

    for spec in &cg.fields {
        let line = generate_field_line(spec, extra_vars);
        if let Some((parent, _)) = spec.path.split_once('.') {
            groups.entry(parent.to_string()).or_default().push(line);
        } else {
            top_level.push(line);
        }
    }

    let mut field_section = String::new();
    for line in &top_level {
        field_section.push_str(line);
        field_section.push('\n');
    }
    for (parent, field_lines) in &groups {
        field_section.push_str(&format!("- {}: object:\n", parent));
        for line in field_lines {
            field_section.push_str(&format!("  {}\n", line));
        }
    }

    format!(
        r#"Generate a character fitting this world:

## World
{world_setting}

## Core Requirements
1. **Diversity**: distinct from typical characters in background, personality, values, speech
2. **Authenticity**: complex motivations, unique speech patterns

## Field Requirements
{field_section}## Output Format
Strict JSON output, no other text."#,
        world_setting = cg.world_setting,
        field_section = field_section,
    )
}
