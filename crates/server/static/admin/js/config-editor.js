// ============================================================================
// Config Editor Functions
// ============================================================================

async function loadConfigList() {
    try {
        var res = await apiFetch(API.CONFIG);
        var files = await res.json();

        var listHtml = files.map(function (f) {
            return '<div data-name="' + escapeHtml(f.name) + '" class="file-item' +
                (currentFile === f.name ? " active" : "") + '">' +
                escapeHtml(f.name) + '<div style="font-size: 10px; color: var(--text-subtle);">' +
                (f.size / 1024).toFixed(1) + " KB</div></div>";
        }).join("");

        document.getElementById("config-list").innerHTML = listHtml;
    } catch (e) {
        if (e.name !== "ApiError") {
            console.error(e);
            showToast("加载文件列表失败", "error");
        }
    }
}

async function loadConfigContent(filename) {
    currentFile = filename;
    document.getElementById("current-file-name").textContent = filename;

    document.querySelectorAll(".file-item").forEach(function (el) {
        el.classList.remove("active");
        if (el.dataset.name === filename) el.classList.add("active");
    });

    try {
        var res = await apiFetch(API.CONFIG + "/" + encodeURIComponent(filename));
        if (res.ok) {
            var data = await res.json();
            document.getElementById("code-editor").value = data.content;
            var saveBtn = document.getElementById("save-config-btn");
            if (saveBtn && typeof authTokenType !== "undefined") {
                saveBtn.disabled = authTokenType !== "write";
            }
        }
    } catch (e) {
        if (e.name !== "ApiError") {
            console.error(e);
            showToast("加载文件内容失败", "error");
        }
    }
}

document.addEventListener("click", function (event) {
    var item = event.target.closest(".file-item[data-name]");
    if (!item) return;
    loadConfigContent(item.dataset.name);
});

async function saveConfig() {
    if (!currentFile) return;

    if (typeof authTokenType !== "undefined" && authTokenType !== "write") {
        showToast("只读 Token 无法保存修改", "error");
        return;
    }

    var content = document.getElementById("code-editor").value;

    if (currentFile.endsWith(".json")) {
        try {
            JSON.parse(content);
        } catch (e) {
            showToast("JSON 语法错误，请检查后再保存", "error");
            return;
        }
    } else if (currentFile.endsWith(".yaml") || currentFile.endsWith(".yml")) {
        try {
            jsyaml.load(content);
        } catch (e) {
            showToast("YAML 语法错误，请检查后再保存", "error");
            return;
        }
    }

    try {
        var res = await apiFetch(API.CONFIG + "/" + encodeURIComponent(currentFile), {
            method: "PUT",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ content: content }),
        });

        if (res.ok) {
            showToast("保存成功！配置已热更新", "success");
        } else {
            var errorText = await res.text();
            showToast("保存失败: " + errorText, "error");
        }
    } catch (e) {
        if (e.name !== "ApiError") {
            console.error(e);
            showToast("网络请求失败", "error");
        }
    }
}
