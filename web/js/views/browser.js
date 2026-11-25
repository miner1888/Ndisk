// 文件浏览:工具栏 / 网格与列表渲染 / 选中 / 悬停菜单 / 键盘 / 手势 / 底部悬浮栏。
import { api } from "../core/api.js";
import { session, esc, fmtSize, fmtTime, childPath, MOVE_MIME } from "../core/state.js";
import { ICN, fileIcon, kindOf } from "../core/icons.js";
import { toast, openMenuAt, closeMenu } from "../core/ui.js";
import { syncSidebar } from "./sidebar.js";

export { bindDropUpload } from "./uploader.js";

let loadSeq = 0;

export async function loadFiles() {
  const el = document.getElementById("files");
  const s = session.share;
  session.albumView = false;
  clearSelection();
  const seq = ++loadSeq;
  if (!s) {
    el.innerHTML = '<div class="empty"><span class="empty-ico">' + ICN.folder + '</span>没有可访问的空间</div>';
    return;
  }
  el.innerHTML = '<div class="center"><div class="spinner"></div></div>';
  try {
    const r = await api("GET", `/api/browse/${s.id}?path=${encodeURIComponent(session.path)}`);
    if (seq !== loadSeq) return;
    session.path = r.path || "";
    session.can = r.can || {};
    renderToolbar();
    renderFiles(r.entries || []);
  } catch (e) {
    if (seq !== loadSeq) return;
    if ((e.message || "").includes("不存在")) {
      session.can = { upload: true, download: true, delete: true, mkdir: true };
      renderToolbar();
      renderFiles([]);
      return;
    }
    el.innerHTML = `<div class="center" style="color:var(--danger)">${esc(e.message)}</div>`;
  }
}

export function renderToolbar() {
  if (session.searchOpen) return renderSearchBar();
  const parts = session.path ? session.path.split("/") : [];
  let crumb = '<a class="crumbs" data-path="" href="#">根目录</a>';
  let acc = "";
  parts.forEach((p) => {
    acc = acc ? acc + "/" + p : p;
    crumb += ` <span class="sp">›</span> <a class="crumbs" data-path="${esc(acc)}" href="#">${esc(p)}</a>`;
  });
  const can = session.can;
  const adm = session.user && session.user.is_admin;
  document.getElementById("toolbar").innerHTML = `
    <button class="btn mini ghost" id="btn-menu" title="菜单">${ICN.menu}</button>
    <div class="crumb">${crumb}</div>
    <input id="Fsearch" class="search-box" placeholder="搜索当前文件夹..." value="${esc(session.filter)}">
    <div class="spacer"></div>
    <button class="btn mini ghost tb-extra" id="tb-view" title="切换视图">${session.view === "grid" ? ICN.list : ICN.grid}</button>
    <button class="btn mini ghost tb-extra" id="tb-sort" title="排序方式">${ICN.list}</button>
    ${adm ? `<button class="btn mini ghost tb-extra" id="tb-perm" title="权限">${ICN.lock}</button>` : ""}
    <button class="btn mini ghost tb-search" id="btn-search" title="搜索">${ICN.search}</button>
    <div class="view-switch" title="切换视图">
      <button class="btn mini ghost view-btn ${session.view === "grid" ? "active" : ""}" data-view="grid">${ICN.grid}</button>
      <button class="btn mini ghost view-btn ${session.view === "list" ? "active" : ""}" data-view="list">${ICN.list}</button>
    </div>
    <select id="sort-by" class="sort-select" title="排序">
      <option value="name" ${session.sort === "name" ? "selected" : ""}>按名称</option>
      <option value="size" ${session.sort === "size" ? "selected" : ""}>按大小</option>
      <option value="mtime" ${session.sort === "mtime" ? "selected" : ""}>按修改时间</option>
      <option value="kind" ${session.sort === "kind" ? "selected" : ""}>按类型</option>
    </select>
    <button class="btn mini ghost" id="btn-selall" title="全选 / 取消全选(Ctrl+A)">全选</button>
    ${can.upload ? `<button class="btn ghost mini" id="btn-offline">${ICN.download}<span class="bt-txt">离线下载</span></button>` : ""}
    ${can.mkdir ? `<button class="btn ghost mini" id="btn-mkdir">${ICN.plus}新建文件夹</button>` : ""}
    ${can.upload ? `<button class="btn btn-upload" id="btn-upload">${ICN.upload}上传文件</button><input type="file" id="file-input" multiple style="display:none">` : ""}
    ${adm ? `<button class="btn mini ghost" id="btn-perm">${ICN.lock}权限</button>` : ""}
    <button class="btn mini ghost" id="btn-refresh" title="刷新">${ICN.refresh}</button>`;
  syncSidebar();
  document.querySelectorAll(".crumbs").forEach((a) => {
    a.onclick = (e) => {
      e.preventDefault();
      session.path = a.dataset.path || "";
      session.filter = "";
      loadFiles();
    };
  });
  document.querySelectorAll(".view-btn").forEach((b) => {
    b.onclick = () => {
      session.view = b.dataset.view;
      session.saveView();
      renderToolbar();
      renderFiles(session.lastEntries);
    };
  });
  document.getElementById("sort-by").onchange = (e) => {
    session.sort = e.target.value;
    session.saveSort();
    renderFiles(session.lastEntries);
  };
  const search = document.getElementById("Fsearch");
  search.oninput = () => {
    session.filter = search.value.trim();
    renderFiles(session.lastEntries);
  };
  const sbt = document.getElementById("btn-search");
  if (sbt) sbt.onclick = () => {
    session.searchOpen = true;
    renderToolbar();
  };
  const vb = document.getElementById("tb-view");
  if (vb) vb.onclick = () => {
    session.view = session.view === "grid" ? "list" : "grid";
    session.saveView();
    renderToolbar();
    renderFiles(session.lastEntries);
  };
  const sob = document.getElementById("tb-sort");
  if (sob) sob.onclick = (ev) => openSortMenu(ev);
  const mk = document.getElementById("btn-mkdir");
  if (mk) mk.onclick = () => import("./dialogs.js").then((m) => m.mkdirModal());
  const sa = document.getElementById("btn-selall");
  if (sa) sa.onclick = toggleSelectAll;
  const up = document.getElementById("btn-upload");
  if (up) up.onclick = () => ensureFileInput().click();
  const pm = document.getElementById("btn-perm");
  if (pm) pm.onclick = () => import("./dialogs.js").then((m) => m.folderPermModal());
  document.getElementById("btn-refresh").onclick = loadFiles;
  const off = document.getElementById("btn-offline");
  if (off) off.onclick = () => import("./offline.js").then((m) => m.offlineModal());
  const mb = document.getElementById("btn-menu");
  if (mb) mb.onclick = toggleSidebar;
  renderFab();
  const fi = document.getElementById("file-input");
  if (fi) {
    fi.onchange = (ev) => {
      const files = Array.from(ev.target.files || []);
      fi.value = "";
      import("./uploader.js").then((m) => m.pickFiles({ target: { files } }));
    };
  }
}

/** 搜索态工具栏:一个输入框 + 取消按钮(移动端) */
function renderSearchBar() {
  const tb = document.getElementById("toolbar");
  tb.innerHTML = `
    <button class="btn mini ghost" id="btn-menu" title="菜单">${ICN.menu}</button>
    <input id="Fsearch" class="search-box" style="display:block" placeholder="搜索当前文件夹..." value="${esc(session.filter)}">
    <button class="btn mini ghost" id="btn-scancel">取消</button>`;
  const s = document.getElementById("Fsearch");
  s.oninput = () => {
    session.filter = s.value.trim();
    renderFiles(session.lastEntries);
  };
  s.onkeydown = (ev) => {
    if (ev.key === "Escape") document.getElementById("btn-scancel").click();
  };
  document.getElementById("btn-scancel").onclick = () => {
    const had = !!session.filter;
    session.searchOpen = false;
    session.filter = "";
    renderToolbar();
    if (had) renderFiles(session.lastEntries);
  };
  const mb = document.getElementById("btn-menu");
  if (mb) mb.onclick = toggleSidebar;
  setTimeout(() => s.focus(), 0);
}

export function ensureFileInput() {
  let i = document.getElementById("file-input");
  if (!i) {
    i = document.createElement("input");
    i.type = "file";
    i.id = "file-input";
    i.multiple = true;
    i.style.display = "none";
    document.body.appendChild(i);
    i.onchange = (ev) => {
      const files = Array.from(ev.target.files || []);
      i.value = "";
      import("./uploader.js").then((m) => m.pickFiles({ target: { files } }));
    };
  }
  return i;
}

export function sortEntries(entries) {
  return entries.slice().sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    switch (session.sort) {
      case "size": return b.size - a.size;
      case "mtime": return b.mtime - a.mtime;
      case "kind": return (a.kind || "").localeCompare(b.kind || "");
      default: return a.name.localeCompare(b.name, "zh-Hans-CN");
    }
  });
}

function renderFiles(entries) {
  session.lastEntries = entries;
  const el = document.getElementById("files");
  closeMenu();
  const shown = session.filter ? entries.filter((x) => x.name.toLowerCase().includes(session.filter.toLowerCase())) : entries;
  if (!shown.length) {
    session.shown = [];
    el.innerHTML = session.filter
      ? `<div class="empty"><span class="empty-ico">${ICN.search}</span><div>未找到与 <b style="color:var(--primary)">${esc(session.filter)}</b> 相关的文件</div></div>`
      : '<div class="empty"><span class="empty-ico">' + ICN.folder + '</span>空文件夹</div>';
    updateSelectAllLabel();
    return;
  }
  const sorted = sortEntries(shown);
  session.shown = sorted;
  const dg = session.can && session.can.delete ? ' draggable="true"' : "";
  if (session.view === "list") {
    el.innerHTML = `<div class="list-view-card">
      <div class="list-caption">共 ${sorted.length} 项${session.filter ? ` · 筛选:"${esc(session.filter)}"` : ""}</div>
      <table class="files-table"><thead><tr><th>名称</th><th>大小</th><th>修改时间</th><th style="width:150px"></th></tr></thead><tbody>
      ${sorted.map((x) => `<tr class="ftr"${dg} data-name="${esc(x.name)}" data-isdir="${x.is_dir ? 1 : 0}" data-kind="${esc(x.kind || "other")}">
        <td class="fname"><label class="sel-cb"><input type="checkbox" class="sel-in"></label><span class="f-ico">${fileIcon(x)}</span><span>${esc(x.name)}</span></td>
        <td>${x.is_dir ? "—" : fmtSize(x.size)}</td>
        <td class="fdate">${fmtTime(x.mtime)}</td>
        <td><span class="ftr-acts">${!x.is_dir && session.can.download ? `<button class="btn mini ghost" data-qdl="${esc(x.name)}" title="下载">${ICN.download}</button>` : ""}
        ${!x.is_dir && session.user ? `<button class="btn mini ghost" data-qshare="${esc(x.name)}" title="分享">${ICN.share}</button>` : ""}
        ${session.can.delete ? `<button class="btn mini ghost" data-qdel="${esc(x.name)}" title="删除">${ICN.trash}</button>` : ""}</span></td>
      </tr>`).join("")}
      </tbody></table></div>`;
    el.querySelectorAll("[data-qdl]").forEach((b) => (b.onclick = () => download(b.dataset.qdl)));
    el.querySelectorAll("[data-qshare]").forEach((b) => (b.onclick = () => import("./dialogs.js").then((m) => m.shareModal(session.share.id, childPath(b.dataset.qshare), b.dataset.qshare))));
    el.querySelectorAll("[data-qdel]").forEach((b) => (b.onclick = () => import("./dialogs.js").then((m) => m.deleteModal([b.dataset.qdel]))));
  } else {
    el.innerHTML = `<div class="grid">${sorted.map((x) => {
      if (x.is_dir) {
        return `<div class="tile dir"${dg} data-name="${esc(x.name)}" data-isdir="1" data-kind="dir">
          <label class="sel-cb"><input type="checkbox" class="sel-in"></label>
          <div class="thumb dir-thumb"><span class="dico">${fileIcon(x)}</span></div>
          <div class="dbelow"><div class="dn" title="${esc(x.name)}">${esc(x.name)}</div><div class="dd">${fmtTime(x.mtime)}</div></div>
        </div>`;
      }
      const isImg = ["jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "avif", "ico"].includes(x.ext);
      const isVid = x.kind === "video" && x.ext !== "avi";
      const kc = kindOf(x);
      const turl = isImg ? `/api/media/imgthumb/${session.share.id}?path=${encodeURIComponent(childPath(x.name))}` : `/api/media/thumb/${session.share.id}?path=${encodeURIComponent(childPath(x.name))}`;
      const showThumb = session.share && (isImg || isVid);
      const thumb = showThumb
        ? `<div class="thumb"><img src="${esc(turl)}" loading="lazy" data-src="${esc(turl)}" data-tr="3" onerror="thumbErr(this)"><span class="tfb">${fileIcon(x)}</span></div>`
        : `<div class="thumb"><span style="filter:drop-shadow(0 6px 10px rgba(24,32,64,.14))">${fileIcon(x)}</span></div>`;
      return `<div class="tile ${kc}"${dg} data-name="${esc(x.name)}" data-isdir="0" data-kind="${esc(x.kind || "other")}">
        <label class="sel-cb"><input type="checkbox" class="sel-in"></label>
        ${thumb}
        <div class="tname" title="${esc(x.name)}">${esc(x.name)}</div>
        <div class="tmeta">${fmtSize(x.size)}</div>
      </div>`;
    }).join("")}</div>`;
  }
  updateSelectAllLabel();
}

/** 文本与表格类型(长按 / 右键菜单里出现"修改")。 */
export function isEditableFile(x) {
  if (!x || x.is_dir) return false;
  const e = (x.ext || "").toLowerCase();
  const TEXT = ["txt", "log", "md", "markdown", "yaml", "yml", "conf", "ini", "toml", "json", "xml", "html", "htm", "css", "js", "mjs", "ts", "sh", "bash", "py", "sql", "properties", "env"];
  const SHEETS = ["xlsx", "xlsm", "xls", "ods", "csv", "tsv"];
  return TEXT.includes(e) || SHEETS.includes(e);
}

function menuItemsFor(x) {
  const items = [];
  if (x.is_dir) {
    if (session.can.download) items.push({ icon: ICN.download, label: "下载", act: () => batchDownload([x.name]) });
  } else if (session.can.download) {
    items.push({ icon: ICN.download, label: "下载", act: () => download(x.name) });
  }
  if (session.can.delete) items.push({ icon: ICN.edit, label: "重命名", act: () => import("./dialogs.js").then((m) => m.renameModal(x.name)) });
  if (session.can.delete) items.push({ icon: ICN.move, label: "移动到...", act: () => import("./dialogs.js").then((m) => m.moveModal([x.name])) });
  if (!x.is_dir && session.user) {
    items.push({ icon: ICN.share, label: "分享", act: () => import("./dialogs.js").then((m) => m.shareModal(session.share.id, childPath(x.name), x.name)) });
    items.push({ icon: ICN.link, label: "复制直链", act: async () => {
      try {
        const r = await api("POST", "/api/file/dlticket", { share_id: session.share.id, path: childPath(x.name), names: [] });
        import("../core/ui.js").then((m2) => m2.copyText(location.origin + r.url, "下载直链已复制"));
      } catch (e) { toast(e.message); }
    } });
  }
  if (isEditableFile(x) && session.user && session.can.upload) {
    items.push({ icon: ICN.save, label: "修改", act: () => openItem(x.name, false, kindOf(x)) });
  }
  items.push({ icon: ICN.info, label: "详情", act: () => import("./dialogs.js").then((m) => m.detailModal(x.name, !!x.is_dir)) });
  if (session.can.delete) items.push({ icon: ICN.trash, label: "删除", act: () => import("./dialogs.js").then((m) => m.deleteModal([x.name])) });
  return items;
}

/** 把工具栏里被收纳的操作渲染进 FAB 菜单;权限变化后由 renderToolbar 重新调用 */
export function renderFab() {
  const wrap = document.getElementById("fabWrap");
  const menu = document.getElementById("fabMenu");
  if (!wrap || !menu) return;
  const can = session.can || {};
  const items = [];
  if (session.user && can.upload) items.push({ icon: ICN.upload, label: "上传文件", act: () => ensureFileInput().click() });
  if (session.user && can.mkdir) items.push({ icon: ICN.plus, label: "新建文件夹", act: () => import("./dialogs.js").then((m) => m.mkdirModal()) });
  if (session.user) {
    items.push({ icon: ICN.download, label: "离线下载", act: () => import("./offline.js").then((m) => m.offlineModal()) });
  }
  menu.innerHTML = items
    .map((it, i) => `<button class="fab-item" data-i="${i}"><span>${esc(it.label)}</span><span class="fi-ico">${it.icon}</span></button>`)
    .join("");
  menu.querySelectorAll(".fab-item").forEach((b) => {
    b.onclick = (ev) => {
      wrap.classList.remove("open");
      const it = items[+b.dataset.i];
      if (it) it.act(ev);
    };
  });
  wrap.classList.toggle("hidden", session.albumView || !session.share || !items.length);
}

/** 排序子菜单:在触发按钮处弹出,当前项打勾 */
function openSortMenu(ev) {
  const opts = [["name", "按名称"], ["size", "按大小"], ["mtime", "按修改时间"], ["kind", "按类型"]];
  const items = opts.map(([v, label]) => ({
    icon: session.sort === v ? ICN.check : "",
    label,
    act: () => {
      session.sort = v;
      session.saveSort();
      renderFiles(session.lastEntries);
      renderFab();
    },
  }));
  openMenuAt(ev, items);
}

document.addEventListener("click", (ev) => {
  const wrap = document.getElementById("fabWrap");
  if (wrap && wrap.classList.contains("open") && !ev.target.closest("#fabWrap")) {
    wrap.classList.remove("open");
  }
});

export function download(name, fullPath) {
  const path = fullPath ? fullPath : childPath(name);
  const a = document.createElement("a");
  a.href = `/api/media/${session.share.id}?path=${encodeURIComponent(path)}&dl=1`;
  a.download = name || "";
  a.className = "dl-anchor";
  document.body.appendChild(a);
  a.click();
  setTimeout(() => a.remove(), 500);
}

export function openItem(name, isdir, kind, fullPath) {
  if (isdir === true || isdir === "1") {
    session.path = childPath(name);
    session.filter = "";
    loadFiles();
    return;
  }
  if (kind === "image" || kind === "video" || kind === "audio") import("./preview.js").then((m) => m.openMedia(name, null, fullPath));
  else import("./preview.js").then((m) => m.openFile(name, null, fullPath));
}

export async function batchDownload(names) {
  if (names.length === 1 && !session.selDir.has(names[0])) {
    download(names[0]);
    toast("开始下载", true);
    return;
  }
  try {
    const r = await api("POST", "/api/file/dlticket", { share_id: session.share.id, path: session.path, names });
    const url = location.origin + r.url;
    const a = document.createElement("a");
    a.href = url;
    a.download = "批量下载.zip";
    a.className = "dl-anchor";
    document.body.appendChild(a);
    a.click();
    setTimeout(() => a.remove(), 500);
    toast("开始下载", true);
  } catch (e) {
    toast(e.message || "打包失败");
  }
}

function itemOf(ev) {
  return ev.target.closest(".tile, .ftr");
}

document.addEventListener("click", (ev) => {
  if (Date.now() < suppressClickUntil) return;
  if (ev.target.closest(".sel-cb")) return;
  if (ev.target.closest(".tile, .ftr")) return;
  if (!ev.target.closest("#selbar, .toolbar, .modal-mask, .share-item, .crumbs, .upload-panel, .dl-anchor, .ctx-menu, #fabWrap")) {
    clearSelection();
  }
});

document.addEventListener("dblclick", (ev) => {
  if (Date.now() < suppressDblclickUntil) return;
  const item = itemOf(ev);
  if (item) openItem(item.dataset.name, item.dataset.isdir === "1", item.dataset.kind);
});

document.addEventListener("change", (ev) => {
  const cb = ev.target.closest(".sel-in");
  if (!cb) return;
  const item = cb.closest(".tile, .ftr");
  if (!item) return;
  const name = item.dataset.name;
  if (cb.checked) {
    session.sel.add(name);
    if (item.dataset.isdir === "1") session.selDir.add(name);
    else session.selDir.delete(name);
  } else {
    session.sel.delete(name);
    session.selDir.delete(name);
  }
  syncSelClass(item);
  renderSelbar();
});

function syncSelClass(item) {
  const on = session.sel.has(item.dataset.name);
  item.classList.toggle("sel", on);
  const cb = item.querySelector(".sel-in");
  if (cb) cb.checked = on;
}

function clearSelection() {
  session.sel.clear();
  session.selDir.clear();
  document.querySelectorAll(".tile, .ftr").forEach((n) => syncSelClass(n));
  renderSelbar();
}

export function selectAll() {
  const list = session.shown || [];
  list.forEach((x) => {
    session.sel.add(x.name);
    if (x.is_dir) session.selDir.add(x.name);
    else session.selDir.delete(x.name);
  });
  document.querySelectorAll(".tile, .ftr").forEach(syncSelClass);
  renderSelbar();
}

function toggleSelectAll() {
  const list = session.shown || [];
  if (!list.length) return;
  if (list.every((x) => session.sel.has(x.name))) {
    list.forEach((x) => {
      session.sel.delete(x.name);
      session.selDir.delete(x.name);
    });
    document.querySelectorAll(".tile, .ftr").forEach(syncSelClass);
    renderSelbar();
  } else {
    selectAll();
  }
}

function updateSelectAllLabel() {
  const btn = document.getElementById("btn-selall");
  if (!btn) return;
  const list = session.shown || [];
  const all = list.length > 0 && list.every((x) => session.sel.has(x.name));
  btn.textContent = all ? "取消全选" : "全选";
  btn.classList.toggle("active", all);
}

// ── 鼠标框选(marquee) ──
let bandEl = null;
let bandStart = null;
let bandBase = new Set();
let bandActive = false;
let suppressClickUntil = 0;

function rectsIntersect(a, b) {
  return a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
}

function applyBand(rect) {
  for (const it of document.querySelectorAll("#files .tile, #files .ftr")) {
    const name = it.dataset.name;
    const hit = bandBase.has(name) || rectsIntersect(rect, it.getBoundingClientRect());
    if (hit) {
      session.sel.add(name);
      if (it.dataset.isdir === "1") session.selDir.add(name);
      else session.selDir.delete(name);
    } else {
      session.sel.delete(name);
      session.selDir.delete(name);
    }
    syncSelClass(it);
  }
  renderSelbar();
}

document.addEventListener("pointerdown", (ev) => {
  if (ev.button !== 0 || session.albumView) return;
  if (ev.pointerType === "touch") return;
  const files = document.getElementById("files");
  if (!files || !files.contains(ev.target)) return;
  if (ev.target.closest(".tile, .ftr, button, a, input, label, .sel-cb, .tile-more, thead")) return;
  bandStart = { x: ev.clientX, y: ev.clientY };
  bandBase = ev.ctrlKey || ev.metaKey ? new Set(session.sel) : new Set();
  bandActive = false;
});

document.addEventListener("pointermove", (ev) => {
  if (!bandStart) return;
  const dx = ev.clientX - bandStart.x;
  const dy = ev.clientY - bandStart.y;
  if (!bandActive && Math.abs(dx) < 5 && Math.abs(dy) < 5) return;
  if (!bandActive) {
    bandActive = true;
    bandEl = document.createElement("div");
    bandEl.className = "band";
    document.body.appendChild(bandEl);
  }
  const rect = {
    left: Math.min(bandStart.x, ev.clientX),
    top: Math.min(bandStart.y, ev.clientY),
    right: Math.max(bandStart.x, ev.clientX),
    bottom: Math.max(bandStart.y, ev.clientY),
  };
  Object.assign(bandEl.style, {
    left: rect.left + "px",
    top: rect.top + "px",
    width: rect.right - rect.left + "px",
    height: rect.bottom - rect.top + "px",
  });
  applyBand(rect);
  const files = document.getElementById("files");
  if (files) {
    const r = files.getBoundingClientRect();
    if (ev.clientY < r.top + 28) files.scrollTop -= 14;
    else if (ev.clientY > r.bottom - 28) files.scrollTop += 14;
  }
});

function endBand() {
  if (!bandStart) return;
  if (bandActive) suppressClickUntil = Date.now() + 350;
  bandStart = null;
  bandActive = false;
  bandBase = new Set();
  if (bandEl) {
    bandEl.remove();
    bandEl = null;
  }
}

document.addEventListener("pointerup", endBand);
document.addEventListener("pointercancel", endBand);

// ── 侧栏抽屉 ──
export function closeSidebar() {
  const sb = document.getElementById("sidebar");
  const mask = document.getElementById("sideMask");
  if (sb) sb.classList.remove("open");
  if (mask) mask.classList.remove("show");
}

function openSidebar() {
  const sb = document.getElementById("sidebar");
  const mask = document.getElementById("sideMask");
  if (sb) sb.classList.add("open");
  if (mask) mask.classList.add("show");
}

function toggleSidebar() {
  const sb = document.getElementById("sidebar");
  const mask = document.getElementById("sideMask");
  if (!sb || !mask) return;
  if (sb.classList.contains("open")) closeSidebar();
  else openSidebar();
}

// ── 内部拖拽移动 ──
let dragNames = [];
let lastDropEl = null;

function clearDropHighlight() {
  if (lastDropEl) {
    lastDropEl.classList.remove("drop-target");
    lastDropEl = null;
  }
}

function dropTargetOf(ev) {
  const dirItem = ev.target.closest(".tile.dir, .ftr");
  if (dirItem && dirItem.dataset.isdir === "1") {
    const name = dirItem.dataset.name;
    if (!dragNames.includes(name)) return { el: dirItem, dest: childPath(name) };
    return null;
  }
  const crumb = ev.target.closest(".crumbs");
  if (crumb) return { el: crumb, dest: crumb.dataset.path || "" };
  return null;
}

document.addEventListener("dragstart", (ev) => {
  const item = ev.target.closest("#files .tile, #files .ftr");
  if (!item) return;
  if (!session.can || !session.can.delete) {
    ev.preventDefault();
    return;
  }
  const name = item.dataset.name;
  dragNames = session.sel.has(name) ? [...session.sel] : [name];
  try {
    ev.dataTransfer.setData(MOVE_MIME, String(dragNames.length));
    ev.dataTransfer.setData("text/plain", dragNames.join("\n"));
  } catch (e) {}
  ev.dataTransfer.effectAllowed = "move";
});

document.addEventListener("dragover", (ev) => {
  if (!dragNames.length) return;
  const t = dropTargetOf(ev);
  clearDropHighlight();
  if (!t) return;
  ev.preventDefault();
  ev.dataTransfer.dropEffect = "move";
  t.el.classList.add("drop-target");
  lastDropEl = t.el;
});

document.addEventListener("drop", (ev) => {
  if (!dragNames.length) return;
  const t = dropTargetOf(ev);
  const names = dragNames;
  dragNames = [];
  clearDropHighlight();
  if (!t) return;
  ev.preventDefault();
  if (t.dest === session.path) return;
  import("./dialogs.js").then((m) => m.moveItems(names, t.dest));
});

document.addEventListener("dragend", () => {
  dragNames = [];
  clearDropHighlight();
});

// ── 返回逻辑(手势 / Edge 滑动) ──
function cdUp() {
  if (!session.share || !session.path) return;
  session.path = session.path.split("/").slice(0, -1).join("/");
  session.filter = "";
  session.searchOpen = false;
  renderToolbar();
  loadFiles();
}

function goBack() {
  if (document.querySelector(".imgviewer")) {
    import("./preview.js").then((m) => m.closeImageViewer()).catch(() => {});
    return;
  }
  if (document.querySelector(".admin-wrap")) {
    import("./shell.js").then((m) => m.renderApp());
    return;
  }
  if (session.albumView) {
    session.path = session.albumReturnPath || "";
    loadFiles();
    return;
  }
  if (session.searchOpen) {
    session.searchOpen = false;
    session.filter = "";
    renderToolbar();
    renderFiles(session.lastEntries);
    return;
  }
  cdUp();
}

// ── 触屏手势(抽屉开合 / 边缘返回 / 文件长按 / 左滑上一级) ──
let gesture = null;
let suppressDblclickUntil = 0;
let lastTouchMenuAt = 0;

const TOUCH_GESTURES = [
  {
    name: "drawer-close",
    test: (ev) => {
      const sb = document.getElementById("sidebar");
      if (!sb || !sb.classList.contains("open") || !ev.target.closest("#sidebar")) return null;
      return { x: ev.clientX, y: ev.clientY };
    },
    onMove: (ev, d) => {
      const dx = ev.clientX - d.x;
      const dy = ev.clientY - d.y;
      if (Math.abs(dy) > Math.abs(dx) + 10) return "cancel";
      if (dx < -48) {
        closeSidebar();
        return "done";
      }
    },
  },
  {
    name: "drawer-open",
    test: (ev) => {
      const sb = document.getElementById("sidebar");
      if (!sb || sb.classList.contains("open") || ev.clientX > 28 || !ev.target.closest(".main")) return null;
      return { x: ev.clientX, y: ev.clientY };
    },
    onMove: (ev, d) => {
      const dx = ev.clientX - d.x;
      const dy = ev.clientY - d.y;
      if (Math.abs(dy) > Math.abs(dx) + 10) return "cancel";
      if (dx > 48) {
        openSidebar();
        return "done";
      }
    },
  },
  {
    name: "edge-back",
    test: (ev) => {
      if (ev.clientX < window.innerWidth - 36) return null;
      if (ev.target.closest("#sidebar, #sideMask, .modal-mask, .imgviewer, .selbar, .upload-panel, #fabWrap, .ctx-menu, .audiobar, .toolbar, a, input, textarea, select")) return null;
      return { x: ev.clientX, y: ev.clientY };
    },
    onMove: (ev, d) => {
      const dx = ev.clientX - d.x;
      const dy = ev.clientY - d.y;
      if (Math.abs(dy) > Math.abs(dx) + 10) return "cancel";
      d.dx = dx;
    },
    onEnd: (ev, d) => {
      if (d.dx !== undefined && d.dx < -56) goBack();
    },
  },
  {
    name: "file-touch",
    test: (ev) => {
      if (session.albumView) return null;
      if (document.querySelector(".admin-wrap, .imgviewer, .modal-mask")) return null;
      if (ev.clientX > window.innerWidth - 36) return null;
      const item = ev.target.closest("#files .tile, #files .ftr");
      if (!item || ev.target.closest(".sel-cb, .tile-more, .ftr-acts, button, a, input, label")) return null;
      const entry = (session.lastEntries || []).find((e) => e.name === item.dataset.name);
      if (!entry) return null;
      return { x: ev.clientX, y: ev.clientY, entry, moved: false, timer: 0 };
    },
    onDown: (d) => {
      d.timer = setTimeout(() => {
        d.timer = 0;
        lastTouchMenuAt = Date.now();
        suppressDblclickUntil = lastTouchMenuAt + 900;
        if (navigator.vibrate) navigator.vibrate(12);
        const items = menuItemsFor(d.entry);
        if (items.length) openMenuAt({ clientX: d.x, clientY: d.y }, items);
      }, 480);
    },
    onMove: (ev, d) => {
      const dx = ev.clientX - d.x;
      const dy = ev.clientY - d.y;
      if (Math.abs(dy) > Math.abs(dx) + 10) return "cancel";
      if (Math.abs(dx) > 10 || Math.abs(dy) > 10) {
        d.moved = true;
        if (d.timer) {
          clearTimeout(d.timer);
          d.timer = 0;
        }
      }
      if (dx < -60) {
        cdUp();
        return "done";
      }
    },
    onEnd: (ev, d) => {
      suppressDblclickUntil = Date.now() + 700;
      if (!d.moved) openItem(d.entry.name, d.entry.is_dir === true, d.entry.kind);
    },
  },
  {
    name: "file-swipe-back",
    test: (ev) => {
      if (session.albumView) return null;
      if (document.querySelector(".admin-wrap, .imgviewer, .modal-mask")) return null;
      if (ev.clientX > window.innerWidth - 36) return null;
      if (ev.target.closest("#sidebar, #sideMask, .modal-mask, .imgviewer, .selbar, .upload-panel, #fabWrap, .ctx-menu, .audiobar, .toolbar, button, a, input, label")) return null;
      if (ev.target.closest("#files .tile, #files .ftr")) return null;
      return { x: ev.clientX, y: ev.clientY };
    },
    onMove: (ev, d) => {
      const dx = ev.clientX - d.x;
      const dy = ev.clientY - d.y;
      if (Math.abs(dy) > Math.abs(dx) + 10) return "cancel";
      if (dx < -60) {
        cdUp();
        return "done";
      }
    },
  },
];

function finishGesture() {
  if (gesture && gesture.data && gesture.data.timer) clearTimeout(gesture.data.timer);
  gesture = null;
}

document.addEventListener("pointerdown", (ev) => {
  if (ev.pointerType !== "touch" || gesture) return;
  for (const g of TOUCH_GESTURES) {
    const data = g.test(ev);
    if (data) {
      gesture = { id: ev.pointerId, name: g.name, g, data };
      if (g.onDown) g.onDown(data);
      return;
    }
  }
});

document.addEventListener("pointermove", (ev) => {
  if (!gesture || ev.pointerId !== gesture.id) return;
  const r = gesture.g.onMove ? gesture.g.onMove(ev, gesture.data) : undefined;
  if (r === "done" || r === "cancel") finishGesture();
});

document.addEventListener("pointerup", (ev) => {
  if (!gesture || ev.pointerId !== gesture.id) return;
  const { g, data } = gesture;
  finishGesture();
  if (g.onEnd) g.onEnd(ev, data);
});

document.addEventListener("pointercancel", finishGesture);

// ── 下拉刷新 ──
let ptr = null;
let ptrEl = null;

function ptrBar() {
  if (!ptrEl) {
    ptrEl = document.createElement("div");
    ptrEl.className = "ptr";
    ptrEl.innerHTML = `${ICN.refresh}<span id="ptrText">下拉刷新</span>`;
    document.body.appendChild(ptrEl);
  }
  return ptrEl;
}

function hidePtr() {
  if (ptrEl) ptrEl.classList.remove("show");
}

document.addEventListener("touchstart", (ev) => {
  const files = document.getElementById("files");
  if (!files || files.scrollTop > 2) return;
  if (ev.target.closest("#sidebar, #sideMask, .modal-mask, .imgviewer, .selbar, .upload-panel, #fabWrap, .ctx-menu, .audiobar, .toolbar, button, a, input, label")) return;
  const t = ev.touches[0];
  ptr = { x: t.clientX, y: t.clientY, armed: false };
});

document.addEventListener(
  "touchmove",
  (ev) => {
    if (!ptr) return;
    const t = ev.touches[0];
    const dy = t.clientY - ptr.y;
    const dx = t.clientX - ptr.x;
    if (Math.abs(dx) > Math.abs(dy) + 10 || dy <= 0) {
      ptr = null;
      hidePtr();
      return;
    }
    ev.preventDefault();
    const dist = Math.min(dy, 140);
    const bar = ptrBar();
    bar.classList.add("show");
    ptr.armed = dist > 70;
    bar.classList.toggle("loading", false);
    const txt = bar.querySelector("#ptrText");
    if (txt) txt.textContent = ptr.armed ? "松手刷新" : "下拉刷新";
    const ico = bar.querySelector(".ui-ico");
    if (ico) ico.style.transform = `rotate(${dist * 2}deg)`;
  },
  { passive: false }
);

document.addEventListener("touchend", () => {
  if (!ptr) return;
  const armed = ptr.armed;
  ptr = null;
  const bar = ptrEl;
  if (!bar || !bar.classList.contains("show")) return;
  const finish = () => setTimeout(() => bar.classList.remove("show", "loading"), 450);
  if (armed) {
    bar.classList.add("loading");
    const txt = bar.querySelector("#ptrText");
    if (txt) txt.textContent = "刷新中...";
    const ico = bar.querySelector(".ui-ico");
    if (ico) ico.style.transform = "";
    const job = session.albumView ? import("./album.js").then((m) => m.loadAlbum()) : Promise.resolve(loadFiles());
    job.finally(finish);
  } else {
    setTimeout(hidePtr, 150);
  }
});

document.addEventListener("touchcancel", () => {
  ptr = null;
  if (ptrEl) ptrEl.classList.remove("show", "loading");
});

/** 底部居中悬浮操作胶囊 */
function renderSelbar() {
  const el = document.getElementById("selbar");
  if (!el) return;
  const n = session.sel.size;
  document.body.classList.toggle("sel-on", n > 0);
  el.classList.toggle("hidden", !n);
  updateSelectAllLabel();
  if (!n) {
    el.innerHTML = "";
    return;
  }
  const can = session.can;
  const names = [...session.sel];
  const one = n === 1 ? names[0] : null;
  el.innerHTML = `
    <span class="sb-count">已选 <b>${n}</b> 项</span>
    <span class="sel-sep"></span>
    ${one ? `<button class="sb-btn" id="sb-open" title="打开">${ICN.folderOpen}</button>` : ""}
    ${can.download ? `<button class="sb-btn" id="sb-dl" title="下载">${ICN.download}</button>` : ""}
    ${can.delete && one ? `<button class="sb-btn" id="sb-rn" title="重命名">${ICN.edit}</button>` : ""}
    ${can.delete ? `<button class="sb-btn" id="sb-move" title="移动">${ICN.move}</button>` : ""}
    ${one ? `<button class="sb-btn" id="sb-info" title="详情">${ICN.info}</button>` : ""}
    ${can.delete ? `<button class="sb-btn danger" id="sb-del" title="删除">${ICN.trash}</button>` : ""}
    <button class="sb-btn sb-close" id="sb-clear" title="取消选择">${ICN.close}</button>`;
  el.querySelector("#sb-clear").onclick = clearSelection;
  const op = el.querySelector("#sb-open");
  if (op) op.onclick = () => openItem(one, session.selDir.has(one), "");
  const dl = el.querySelector("#sb-dl");
  if (dl) dl.onclick = () => batchDownload(names);
  const mv = el.querySelector("#sb-move");
  if (mv) mv.onclick = () => import("./dialogs.js").then((m) => m.moveModal(names));
  const rn = el.querySelector("#sb-rn");
  if (rn && one) rn.onclick = () => import("./dialogs.js").then((m) => m.renameModal(one));
  const info = el.querySelector("#sb-info");
  if (info) info.onclick = () => import("./dialogs.js").then((m) => m.detailModal(one, session.selDir.has(one)));
  const del = el.querySelector("#sb-del");
  if (del) del.onclick = () => import("./dialogs.js").then((m) => m.deleteModal(names));
}

// ── 右键菜单(文件 / 空白处)与键盘快捷键 ──
document.addEventListener("contextmenu", (ev) => {
  if (Date.now() - lastTouchMenuAt < 500) {
    ev.preventDefault();
    return;
  }
  const item = itemOf(ev);
  if (!item) return;
  ev.preventDefault();
  const name = item.dataset.name;
  const isdir = item.dataset.isdir === "1";
  const kind = item.dataset.kind;
  const entry = (session.lastEntries || []).find((e) => e.name === name);
  if (entry) {
    const items = menuItemsFor(entry);
    if (items.length) openMenuAt(ev, items);
  } else {
    const items = [];
    if (!isdir && session.can.download) items.push({ icon: ICN.download, label: "下载", act: () => download(name) });
    if (!isdir) openItem(name, isdir, kind);
    openMenuAt(ev, items);
  }
});

document.addEventListener("contextmenu", (ev) => {
  if (ev.target.closest(".modal-mask, .ctx-menu")) return;
  if (itemOf(ev) || ev.target.closest(".sel-cb")) return;
  ev.preventDefault();
  const items = [];
  if (session.can.upload) items.push({ icon: ICN.upload, label: "上传文件到当前目录", act: () => ensureFileInput().click() });
  if (session.can.mkdir) items.push({ icon: ICN.plus, label: "新建文件夹", act: () => import("./dialogs.js").then((m) => m.mkdirModal()) });
  items.push({ icon: ICN.refresh, label: "刷新", act: loadFiles });
  openMenuAt(ev, items);
});

document.addEventListener("keydown", (ev) => {
  if (ev.target.closest("input, select, textarea")) return;
  if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "a") {
    ev.preventDefault();
    selectAll();
    return;
  }
  if (ev.key === "Delete" && session.sel.size) {
    ev.preventDefault();
    import("./dialogs.js").then((m) => m.deleteModal([...session.sel]));
  } else if (ev.key === "F2" && session.sel.size === 1) {
    ev.preventDefault();
    import("./dialogs.js").then((m) => m.renameModal([...session.sel][0]));
  } else if (ev.key === "Escape") {
    closeMenu();
  }
});

document.addEventListener("keydown", (ev) => {
  if (ev.target.closest("input, select, textarea")) return;
  if (ev.key !== "Enter" || session.sel.size !== 1) return;
  const name = [...session.sel][0];
  openItem(name, session.selDir.has(name), "");
});
