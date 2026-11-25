// 离线下载弹窗:SSE 推送驱动,按任务节点原地更新。
import { api, postForm } from "../core/api.js";
import { session, esc, fmtSize, OFFLINE_STATE_TEXT, OFFLINE_STATE_TEXT_ED2K } from "../core/state.js";
import { ICN } from "../core/icons.js";
import { toast, openModal } from "../core/ui.js";

const fmtSpeed = (b) => {
  if (b == null) return "";
  if (b >= 1048576) return (b / 1048576).toFixed(1) + " MB/s";
  if (b >= 1024) return (b / 1024).toFixed(0) + " KB/s";
  return b + " B/s";
};
const fmtEta = (s) => {
  if (s == null || s < 0) return "";
  return s >= 3600
    ? `${Math.floor(s / 3600)}时${Math.floor((s % 3600) / 60)}分`
    : s >= 60
      ? `${Math.floor(s / 60)}分${s % 60}秒`
      : `${s}秒`;
};

export function offlineModal() {
  const home = session.shares.find((s) => s.kind === "home" && s.mine) || session.share;
  if (!home) return toast("没有可用的个人空间");

  const tasks = new Map();
  let es = null;
  let badgeTimer = null;
  let selectorShownId = null;

  const mask = openModal(
    `
    <div class="off">
      <div class="off-add">
        <textarea id="off-url" rows="3" placeholder="粘贴链接,每行一个\n支持 HTTP / 磁力 / 迅雷 / 电驴(ed2k)/ .torrent 直链..." autocomplete="off" spellcheck="false"></textarea>
        <div class="off-add-bar">
          <span class="off-add-hint">支持一次添加多个链接</span>
          <input type="file" id="off-tfile" accept=".torrent,application/x-bittorrent" hidden>
          <button class="off-add-btn" id="off-torrent">上传种子</button>
          <button class="off-add-btn" id="off-add">添加</button>
        </div>
      </div>
      <div class="off-hint">保存到 个人空间 / 下载 · 磁力 / 种子会先解析文件清单供你勾选 · 默认不做种</div>
      <div id="off-selector" class="off-selector hidden"></div>
      <div class="off-list" id="off-tasks"><div class="off-empty">暂无任务,粘贴上方链接开始离线下载</div></div>
    </div>`,
    "modal-md"
  );
  mask.querySelector(".mhead .mt").textContent = "离线下载";
  const urlInp = mask.querySelector("#off-url");
  const listEl = mask.querySelector("#off-tasks");
  const selEl = mask.querySelector("#off-selector");

  const taskHtml = (t) => {
    const pct = t.size > 0 ? Math.min(100, Math.round((t.downloaded / t.size) * 100)) : 0;
    const showBar = t.state === "downloading" || t.state === "completed";
    let badge = (t.kind === "ed2k" && OFFLINE_STATE_TEXT_ED2K[t.state]) || OFFLINE_STATE_TEXT[t.state] || t.state;
    if (t.createdAt && ["parsing", "awaiting_select", "downloading"].includes(t.state)) {
      const secs = Math.max(0, Math.round(Date.now() / 1000 - t.createdAt));
      badge += secs >= 60 ? ` ${Math.floor(secs / 60)}分${secs % 60}秒` : ` ${secs}秒`;
    }
    const bar = showBar
      ? `<div class="off-task-bar">
          <div class="up-bar"><div class="up-fill" style="width:${pct}%"></div></div>
          <span class="off-size">${t.size > 0 ? `${fmtSize(t.downloaded)} / ${fmtSize(t.size)}` : fmtSize(t.downloaded)}${t.speed ? ` · ${fmtSpeed(t.speed)}` : ""}${t.eta != null ? ` · 剩余 ${fmtEta(t.eta)}` : ""}</span>
        </div>`
      : "";
    const err = t.error ? `<div class="off-err">${esc(t.error)}</div>` : "";
    const choose =
      t.state === "awaiting_select"
        ? `<button class="btn mini off-choose" data-id="${t.id}">选择要下载的文件</button>`
        : "";
    // 服务重启中断的任务:给出重试入口,一键用原始链接重新下载。
    const retry =
      t.state === "failed" && (t.error || "").includes("服务重启")
        ? `<button class="btn mini off-retry" data-id="${t.id}">重新下载</button>`
        : "";
    return `<div class="off-task ${t.state}" data-id="${t.id}">
      <div class="off-task-head">
        <span class="off-ico">${t.kind === "bittorrent" ? ICN.link : ICN.download}</span>
        <span class="off-name" title="${esc(t.name || t.url)}">${esc(t.name || t.url)}</span>
        <span class="off-badge">${esc(badge)}</span>
        <button class="off-cancel" data-id="${t.id}" title="取消">${ICN.close}</button>
      </div>
      ${bar}${err}${retry}${choose}
    </div>`;
  };

  const bindNode = (node, t) => {
    node.querySelector(".off-cancel").onclick = () => cancelTask(t.id);
    node.querySelector(".off-retry")?.addEventListener("click", () => retryTask(t.id));
    node.querySelector(".off-choose")?.addEventListener("click", () => {
      const cur = tasks.get(t.id);
      if (cur) {
        selectorShownId = cur.id;
        showSelector(cur);
      }
    });
  };

  const retryTask = async (id) => {
    try {
      await api("POST", "/api/offline/retry", { id });
      toast("已重新下载", true);
      syncAll();
    } catch (e) {
      toast(e.message);
    }
  };

  const mountTask = (t) => {
    tasks.set(t.id, t);
    listEl.querySelector(".off-empty")?.remove();
    const wrap = document.createElement("div");
    wrap.innerHTML = taskHtml(t);
    const node = wrap.firstElementChild;
    bindNode(node, t);
    // 活跃任务排在前面。
    const weight = (s) => (["downloading", "awaiting_select", "parsing"].includes(s) ? 0 : 1);
    const after = [...listEl.children].find((n) => weight(tasks.get(Number(n.dataset.id))?.state) > weight(t.state));
    listEl.insertBefore(node, after || null);
  };

  const patchTask = (id, patch) => {
    const t = tasks.get(id);
    if (!t) return;
    Object.assign(t, patch);
    const node = listEl.querySelector(`.off-task[data-id="${id}"]`);
    if (!node) {
      mountTask(t);
      return;
    }
    const fill = node.querySelector(".up-fill");
    if (fill && t.size > 0) {
      fill.style.width = `${Math.min(100, Math.round((t.downloaded / t.size) * 100))}%`;
    }
    const sizeEl = node.querySelector(".off-size");
    if (sizeEl) {
      sizeEl.textContent =
        (t.size > 0 ? `${fmtSize(t.downloaded)} / ${fmtSize(t.size)}` : fmtSize(t.downloaded)) +
        (t.speed ? ` · ${fmtSpeed(t.speed)}` : "") +
        (t.eta != null ? ` · 剩余 ${fmtEta(t.eta)}` : "");
    }
    if (patch.state != null || patch.error != null) {
      node.outerHTML = taskHtml(t);
      bindNode(listEl.querySelector(`.off-task[data-id="${id}"]`), t);
    }
  };

  const removeTask = (id) => {
    tasks.delete(id);
    listEl.querySelector(`.off-task[data-id="${id}"]`)?.remove();
    if (selectorShownId === id) {
      selEl.classList.add("hidden");
      selectorShownId = null;
    }
    if (!listEl.children.length) {
      listEl.innerHTML = '<div class="off-empty">暂无任务,粘贴上方链接开始离线下载</div>';
    }
  };

  /** 全量对齐:以服务端列表为准增删节点。 */
  const syncAll = async () => {
    try {
      const r = await api("GET", "/api/offline/list");
      const seen = new Set();
      for (const t of r.tasks || []) {
        seen.add(t.id);
        if (tasks.has(t.id)) patchTask(t.id, t);
        else mountTask(t);
      }
      for (const id of [...tasks.keys()]) {
        if (!seen.has(id)) removeTask(id);
      }
      autoShowSelector();
    } catch {
      /* 拉取失败:等 SSE 下一帧或手动刷新 */
    }
  };

  const autoShowSelector = () => {
    const wait = [...tasks.values()].find((t) => t.state === "awaiting_select");
    if (wait && selectorShownId !== wait.id) {
      selectorShownId = wait.id;
      showSelector(wait);
    } else if (!wait && selectorShownId != null) {
      selEl.classList.add("hidden");
      selectorShownId = null;
    }
  };

  const connect = () => {
    const token = session.token ? `?token=${encodeURIComponent(session.token)}` : "";
    es = new EventSource(`/api/offline/events${token}`);
    es.onopen = () => syncAll();
    es.onmessage = (ev) => {
      let msg;
      try {
        msg = JSON.parse(ev.data);
      } catch {
        return;
      }
      if (msg.type === "snapshot") return syncAll();
      if (msg.type === "resync") return syncAll();
      if (msg.type === "update") {
        if (!tasks.has(msg.id)) return syncAll();
        patchTask(msg.id, msg.patch || {});
        autoShowSelector();
      } else if (msg.type === "removed") {
        removeTask(msg.id);
      }
    };
  };

  // 进行中任务的徽章每秒自动累加耗时。
  badgeTimer = setInterval(() => {
    for (const t of tasks.values()) {
      if (!["parsing", "awaiting_select", "downloading"].includes(t.state)) continue;
      const node = listEl.querySelector(`.off-task[data-id="${t.id}"] .off-badge`);
      if (!node) continue;
      let badge = (t.kind === "ed2k" && OFFLINE_STATE_TEXT_ED2K[t.state]) || OFFLINE_STATE_TEXT[t.state] || t.state;
      const secs = Math.max(0, Math.round(Date.now() / 1000 - t.createdAt));
      badge += secs >= 60 ? ` ${Math.floor(secs / 60)}分${secs % 60}秒` : ` ${secs}秒`;
      node.textContent = badge;
    }
  }, 1000);

  const cancelTask = async (id) => {
    const t = tasks.get(id);
    try {
      if (t && ["completed", "failed", "cancelled"].includes(t.state)) {
        await api("POST", "/api/offline/remove", { id });
        removeTask(id);
      } else {
        await api("POST", "/api/offline/cancel", { id });
        patchTask(id, { state: "cancelled" });
        autoShowSelector();
        toast("已取消", true);
      }
    } catch (e) {
      toast(e.message);
    }
  };

  const addTask = async () => {
    const links = urlInp.value.split(/\n/).map((s) => s.trim()).filter(Boolean);
    if (!links.length) return toast("请输入链接");
    let added = 0;
    for (const url of links) {
      try {
        await api("POST", "/api/offline/add", { dir: "下载", url });
        added++;
      } catch (e) {
        toast(`${url.slice(0, 40)}...: ${e.message}`);
      }
    }
    urlInp.value = "";
    if (added) toast(`已添加 ${added} 个任务`, true);
    syncAll();
  };
  mask.querySelector("#off-add").onclick = addTask;
  urlInp.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") addTask();
  });

  // 上传种子文件:客户端预检大小,服务端做内容级校验(真正的解析在任务第一阶段)。
  const tfile = mask.querySelector("#off-tfile");
  mask.querySelector("#off-torrent").onclick = () => tfile.click();
  tfile.onchange = async () => {
    const file = tfile.files[0];
    tfile.value = "";
    if (!file) return;
    if (file.size > 16 * 1024 * 1024) return toast("种子文件需在 16MB 以内");
    const fd = new FormData();
    fd.append("dir", "下载");
    fd.append("file", file, file.name);
    try {
      await postForm("/api/offline/add-torrent", fd);
      toast("种子任务已添加", true);
      syncAll();
    } catch (e) {
      toast(e.message);
    }
  };

  const showSelector = (task) => {
    let raw = task.files || [];
    if (typeof raw === "string") {
      try {
        raw = JSON.parse(raw);
      } catch {
        raw = [];
      }
    }
    const files = (Array.isArray(raw) ? raw : []).filter((f) => typeof f.idx === "number");
    // 按顶级目录分组展示。
    const groups = {};
    files.forEach((f) => {
      const parts = (f.path || "").split("/");
      const top = parts.length > 1 ? parts[0] : "(根目录)";
      (groups[top] = groups[top] || []).push(f);
    });
    selEl.classList.remove("hidden");
    selEl.innerHTML = `
      <div class="sel-head">
        <span>共 ${files.length} 个文件 · 总大小 ${fmtSize(files.reduce((a, f) => a + (f.size || 0), 0))}</span>
        <label class="check"><input type="checkbox" id="sel-all" checked> 全选</label>
      </div>
      <div class="sel-tree">
        ${Object.entries(groups)
          .map(
            ([g, fs]) => `
          <div class="sel-group"><div class="sel-dir"><label class="check"><input type="checkbox" class="sel-g" data-g="${esc(g)}" checked> ${esc(g)}</label></div>
          ${fs
            .map(
              (f) => `
            <div class="sel-file"><label class="check"><input type="checkbox" class="sel-f" data-idx="${f.idx}" data-g="${esc(g)}" checked> ${esc((f.path || "").split("/").pop())}</label><span class="sel-size">${fmtSize(f.size || 0)}</span></div>`
            )
            .join("")}
          </div>`
          )
          .join("")}
      </div>
      <div class="help-p">未勾选的文件不会下载、不占磁盘。</div>
      <div class="help-actions"><button class="btn" id="sel-ok">开始下载</button></div>`;
    const refreshOk = () => {
      selEl.querySelector("#sel-ok").disabled = ![...selEl.querySelectorAll(".sel-f")].some((f) => f.checked);
    };
    selEl.querySelector("#sel-all").onchange = (ev) => {
      const on = ev.target.checked;
      selEl.querySelectorAll(".sel-g").forEach((g) => (g.checked = on));
      selEl.querySelectorAll(".sel-f").forEach((f) => (f.checked = on));
      refreshOk();
    };
    selEl.querySelectorAll(".sel-g").forEach((cb) => {
      cb.onchange = () => {
        selEl.querySelectorAll(`.sel-f[data-g="${CSS.escape(cb.dataset.g)}"]`).forEach((f) => {
          f.checked = cb.checked;
        });
        refreshOk();
      };
    });
    selEl.querySelectorAll(".sel-f").forEach((f) => (f.onchange = refreshOk));
    refreshOk();
    selEl.querySelector("#sel-ok").onclick = async () => {
      const selected = [...selEl.querySelectorAll(".sel-f:checked")].map((f) => Number(f.dataset.idx));
      if (!selected.length) return toast("请勾选要下载的文件");
      try {
        await api("POST", "/api/offline/select", { id: task.id, selected });
        toast("已开始下载", true);
        selEl.classList.add("hidden");
        selectorShownId = null;
        syncAll();
      } catch (e) {
        toast(e.message);
      }
    };
  };

  // 弹窗被移除时收尾(关 SSE、停计时器);页面重新可见时全量对齐一次。
  const close = () => {
    es?.close();
    es = null;
    if (badgeTimer) clearInterval(badgeTimer);
    badgeTimer = null;
    document.removeEventListener("visibilitychange", onVis);
  };
  const observer = new MutationObserver(() => {
    if (!document.body.contains(mask)) close();
  });
  observer.observe(document.body, { childList: true });
  const onVis = () => {
    if (!document.hidden) syncAll();
  };
  document.addEventListener("visibilitychange", onVis);

  connect();
  syncAll();
}
