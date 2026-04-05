// ============================================================================
// Config Editor Functions
// ============================================================================

async function loadConfigList() {
    try {
        var res = await fetch("/api/config", { headers: getAuthHeaders() });
        if (handleAuthError(res)) return;
        var files = await res.json();

        var listHtml = files.map(function (f) {
            return '<div data-name="' + escapeHtml(f.name) + '" onclick="loadConfigContent(this.dataset.name)" class="file-item' +
                (currentFile === f.name ? " active" : "") + '">' +
                escapeHtml(f.name) + '<div style="font-size: 10px; color: #999;">' +
                (f.size / 1024).toFixed(1) + " KB</div></div>";
        }).join("");

        document.getElementById("config-list").innerHTML = listHtml;
    } catch (e) {
        console.error(e);
        showToast("加载文件列表失败", "error");
    }
}

async function loadConfigContent(filename) {
    currentFile = filename;
    document.getElementById("current-file-name").textContent = filename;

    document.querySelectorAll(".file-item").forEach(function (el) {
        el.classList.remove("active");
        if (el.textContent.includes(filename)) el.classList.add("active");
    });

    try {
        var res = await fetch("/api/config/" + filename, { headers: getAuthHeaders() });
        if (handleAuthError(res)) return;
        var data = await res.json();
        document.getElementById("code-editor").value = data.content;
    } catch (e) {
        showToast("加载文件内容失败", "error");
    }
}

async function saveConfig() {
    if (!currentFile) return;

    var content = document.getElementById("code-editor").value;

    if (currentFile.endsWith(".json")) {
        try {
            JSON.parse(content);
        } catch (e) {
            showToast("JSON 语法错误，请检查后再保存", "error");
            return;
        }
    }

    try {
        var headers = Object.assign({ "Content-Type": "application/json" }, getAuthHeaders());
        var res = await fetch("/api/config/" + currentFile, {
            method: "PUT",
            headers: headers,
            body: JSON.stringify({ content: content }),
        });

        if (res.ok) {
            showToast("保存成功！配置已热更新", "success");
        } else {
            if (handleAuthError(res)) return;
            var errorText = await res.text();
            showToast("保存失败: " + errorText, "error");
        }
    } catch (e) {
        console.error(e);
        showToast("网络请求失败", "error");
    }
}
