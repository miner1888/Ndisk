// 预览与在线编辑:媒体播放 / 文本(编码检测)/ 表格(保留格式与合并单元格)/ docx / pptx 大纲 / pdf
import { api } from "./api.js";
import { session, esc, fmtSize, childPath, mediaUrl } from "./store.js";
import { ICN, fileIcon } from "./icons.js";
import { toast, openModal } from "./ui.js";
import { sortEntries, download, isEditableFile } from "./browser.js";

let pvList = [];
let pvTimer = null;

function pvCancel() {
  if (pvTimer) {
    clearTimeout(pvTimer);
    pvTimer = null;
  }
}

function pvShell() {
  const mask = openModal(`
    <div class="mbody" id="pv-body" style="padding:16px;display:flex;align-items:center;justify-content:center;min-height:180px"></div>
    <div class="meta">
      <span class="pv-nav" id="pv-nav"></span>
      <b id="pv-name" style="font-size:13px"></b>
      <span id="pv-size"></span>
      ${session.user ? '<button class="btn mini ghost" data-share>' + ICN.share + '分享</button>' : ""}
      <button class="btn mini" data-dl style="margin-left:auto">下载</button>
    </div>`, "pv");
  return mask;
}

function pvPrepare(mask, name) {
  if (!mask) {
    pvList = sortEntries(session.lastEntries).filter((e) => !e.is_dir);
    mask = pvShell();
  }
  mask.dataset.cur = name;
  const nEl = mask.querySelector("#pv-name");
  if (nEl) nEl.textContent = name;
  const tEl = mask.querySelector(".mhead .mt");
  if (tEl) tEl.textContent = name;
  return mask;
}

function pvNav(dir) {
  const pv = document.querySelector(".modal.pv");
  const mask = pv ? pv.closest(".modal-mask") : null;
  if (!mask) return;
  const idx = pvList.findIndex((e) => e.name === mask.dataset.cur);
  const next = idx + dir;
  if (next < 0 || next >= pvList.length) return;
  const target = pvList[next];
  if (!target) return;
  if (target.kind === "image" || target.kind === "video" || target.kind === "audio") openMedia(target.name, mask);
  else openFile(target.name, mask);
}

// ---------------- 图片整页查看器(可切换全部图片) ----------------

let viewer = null;

/** 当前可切换的图片清单:相册里为相册全部图片,目录里为当前目录的图片 */
function currentImageItems(fullPath, name) {
  let items = [];
  if (session.albumView && Array.isArray(session.albumImages) && session.albumImages.length) {
    items = session.albumImages.map((i) => ({ path: i.path, name: i.name }));
  } else {
    const list = session.shown && session.shown.length ? session.shown : sortEntries(session.lastEntries || []);
    items = list.filter((e) => !e.is_dir && e.kind === "image").map((e) => ({ path: childPath(e.name), name: e.name }));
  }
  if (!items.some((i) => i.path === fullPath)) items.unshift({ path: fullPath, name });
  return items;
}

export function openImageViewer(name, fullPath) {
  closeImageViewer();
  const items = currentImageItems(fullPath, name);
  if (!items.length) return;
  const albumMode = !!session.albumView;
  const el = document.createElement("div");
  el.className = "imgviewer" + (albumMode ? " album-mode" : "");
  el.innerHTML = `
    <div class="iv-top">
      <span class="iv-name"></span>
      <span class="iv-count"></span>
      <span class="iv-spacer"></span>
      <button class="iv-btn" data-iv="dl" title="下载">${ICN.download}</button>
      <button class="iv-btn" data-iv="info" title="详情">${ICN.info}</button>
      <button class="iv-btn" data-iv="close" title="关闭 (Esc)">${ICN.close}</button>
    </div>
    <button class="iv-nav iv-prev" type="button" title="上一张">${ICN.chevL}</button>
    <div class="iv-stage"><img id="iv-img" alt=""></div>
    <button class="iv-nav iv-next" type="button" title="下一张">${ICN.chevR}</button>`;
  document.body.appendChild(el);

  const stage = el.querySelector(".iv-stage");
  const img = el.querySelector("#iv-img");
  const box = { el, items, idx: Math.max(0, items.findIndex((i) => i.path === fullPath)) };
  viewer = box;

  const show = (next) => {
    const n = items.length;
    box.idx = ((next % n) + n) % n;
    const cur = items[box.idx];
    img.src = mediaUrl(session.share.id, cur.path);
    el.querySelector(".iv-name").textContent = albumMode ? "" : cur.name;
    el.querySelector(".iv-count").textContent = n > 1 ? `${box.idx + 1} / ${n}` : "";
    el.classList.toggle("single", n <= 1);
    // 预加载前后各一张,切换更跟手
    [1, n - 1].forEach((d) => {
      const nb = items[(box.idx + d) % n];
      if (nb && nb.path !== cur.path) new Image().src = mediaUrl(session.share.id, nb.path);
    });
  };

  el.querySelector('[data-iv="close"]').onclick = closeImageViewer;
  el.querySelector('[data-iv="dl"]').onclick = () => {
    const c = items[box.idx];
    download(c.name, c.path);
  };
  el.querySelector('[data-iv="info"]').onclick = () => {
    const c = items[box.idx];
    import("./dialogs.js").then((m) => m.detailModal(c.name, false, c.path));
  };
  el.querySelector(".iv-prev").onclick = () => show(box.idx - 1);
  el.querySelector(".iv-next").onclick = () => show(box.idx + 1);
  // 点击空白处关闭(点图片本身不关闭)
  stage.onclick = (ev) => {
    // 刚完成滑动切换时不当作点击,避免误关查看器
    if (Date.now() - lastSwipeAt < 400) return;
    if (ev.target === stage) closeImageViewer();
  };
  // 滚轮切换:上下 / 左右方向均可,带节流避免触控板连跳
  let lastWheel = 0;
  el.addEventListener(
    "wheel",
    (ev) => {
      const d = Math.abs(ev.deltaY) >= Math.abs(ev.deltaX) ? ev.deltaY : ev.deltaX;
      if (!d) return;
      ev.preventDefault();
      const now = Date.now();
      if (now - lastWheel < 220) return;
      lastWheel = now;
      show(box.idx + (d > 0 ? 1 : -1));
    },
    { passive: false }
  );
  // 触屏滑动:按住图片左滑下一张,右滑上一张(与左右按钮一致),拖动过程图片跟手;
  // 从屏幕右缘向左滑 = 返回(关闭查看器),单张图片也可用
  let swipe = null;
  let lastSwipeAt = 0;
  stage.addEventListener("pointerdown", (ev) => {
    if (ev.pointerType !== "touch") return;
    swipe = { x: ev.clientX, y: ev.clientY, moved: false, edge: ev.clientX > window.innerWidth - 36, multi: items.length > 1 };
  });
  stage.addEventListener("pointermove", (ev) => {
    if (!swipe) return;
    // 单张且非右缘手势:没有可滑动语义,不做跟手
    if (!swipe.multi && !swipe.edge) return;
    const dx = ev.clientX - swipe.x;
    const dy = ev.clientY - swipe.y;
    if (Math.abs(dy) > Math.abs(dx) + 10) {
      swipe = null;
      img.style.transform = "";
      return;
    }
    if (Math.abs(dx) > 8) swipe.moved = true;
    img.style.transition = "none";
    img.style.transform = `translateX(${dx * 0.5}px)`;
  });
  stage.addEventListener("pointerup", (ev) => {
    if (!swipe) return;
    const dx = ev.clientX - swipe.x;
    const moved = swipe.moved;
    const edge = swipe.edge;
    const multi = swipe.multi;
    swipe = null;
    img.style.transition = "";
    img.style.transform = "";
    if (edge && dx < -50) {
      lastSwipeAt = Date.now();
      closeImageViewer();
      return;
    }
    if (!multi || !moved || Math.abs(dx) < 50) return;
    lastSwipeAt = Date.now();
    if (dx < 0) show(box.idx + 1);
    else show(box.idx - 1);
  });
  stage.addEventListener("pointercancel", () => {
    swipe = null;
    img.style.transition = "";
    img.style.transform = "";
  });

  const onKey = (ev) => {
    if (ev.key === "Escape") {
      ev.preventDefault();
      closeImageViewer();
    } else if (ev.key === "ArrowLeft") {
      ev.preventDefault();
      show(box.idx - 1);
    } else if (ev.key === "ArrowRight") {
      ev.preventDefault();
      show(box.idx + 1);
    }
  };
  box.onKey = onKey;
  document.addEventListener("keydown", onKey);
  show(box.idx);
}

export function closeImageViewer() {
  if (!viewer) return;
  document.removeEventListener("keydown", viewer.onKey);
  viewer.el.remove();
  viewer = null;
}

// ---------------- 底部常驻音频播放器(播放时可继续浏览) ----------------

let audioEl = null;
let audioBar = null;
let audioTrack = null;
let audioList = [];
let audioIdx = -1;
let seeking = false;

function fmtClock(sec) {
  if (!isFinite(sec) || sec < 0) sec = 0;
  const s = Math.floor(sec % 60);
  const m = Math.floor(sec / 60);
  return `${m}:${String(s).padStart(2, "0")}`;
}

function ensureAudioEl() {
  if (audioEl) return audioEl;
  audioEl = document.createElement("audio");
  audioEl.preload = "metadata";
  audioEl.style.display = "none";
  document.body.appendChild(audioEl);
  ["timeupdate", "play", "pause", "ended", "loadedmetadata", "durationchange"].forEach((t) =>
    audioEl.addEventListener(t, syncAudioBar)
  );
  return audioEl;
}

/** 播放列表 = 当前目录内的音频(按当前排序);不在列表中的单独文件也放进去 */
function buildAudioList(path, name) {
  const list = session.shown && session.shown.length ? session.shown : sortEntries(session.lastEntries || []);
  let items = list.filter((e) => !e.is_dir && e.kind === "audio").map((e) => ({ path: childPath(e.name), name: e.name }));
  let i = items.findIndex((x) => x.path === path);
  if (i < 0) {
    items = [{ path, name }].concat(items);
    i = 0;
  }
  audioList = items;
  audioIdx = i;
}

export function playAudio(shareId, path, name) {
  const a = ensureAudioEl();
  const same = audioTrack && audioTrack.shareId === shareId && audioTrack.path === path;
  if (!same || !audioList.length) {
    buildAudioList(path, name);
  } else {
    const i = audioList.findIndex((x) => x.path === path);
    if (i >= 0) audioIdx = i;
  }
  audioTrack = { shareId, path, name };
  if (!same) a.src = mediaUrl(shareId, path);
  a.play().catch((e) => toast("无法播放: " + (e.message || e.name)));
  renderAudioBar();
}

/** 上一首 / 下一首:在当前目录的音频里循环 */
function playSibling(dir) {
  if (!audioTrack || audioList.length < 2) return;
  const n = audioList.length;
  audioIdx = ((audioIdx + dir) % n + n) % n;
  const t = audioList[audioIdx];
  const a = ensureAudioEl();
  audioTrack = { shareId: audioTrack.shareId, path: t.path, name: t.name };
  a.src = mediaUrl(audioTrack.shareId, t.path);
  a.play().catch(() => {});
  renderAudioBar();
}

function renderAudioBar() {
  if (!audioBar) {
    audioBar = document.createElement("div");
    audioBar.id = "audiobar";
    audioBar.className = "audiobar";
    audioBar.innerHTML = `
      <span class="ab-grip" title="拖动移动">${ICN.dots}</span>
      <button class="ab-play" type="button" title="播放 / 暂停">${ICN.play}</button>
      <div class="ab-info">
        <div class="ab-title"></div>
        <input class="ab-seek" type="range" min="0" max="1000" value="0" step="1" title="进度">
      </div>
      <span class="ab-time">0:00 / 0:00</span>
      <button class="ab-btn" data-ab="prev" title="上一首">${ICN.chevL}</button>
      <button class="ab-btn" data-ab="next" title="下一首">${ICN.chevR}</button>
      <button class="ab-btn" data-ab="dl" title="下载">${ICN.download}</button>
      <button class="ab-btn" data-ab="close" title="关闭播放器">${ICN.close}</button>`;
    document.body.appendChild(audioBar);
    document.body.classList.add("audio-on");
    audioBar.querySelector(".ab-play").onclick = () => {
      const a = ensureAudioEl();
      if (a.paused) a.play().catch(() => {});
      else a.pause();
    };
    audioBar.querySelector('[data-ab="prev"]').onclick = () => playSibling(-1);
    audioBar.querySelector('[data-ab="next"]').onclick = () => playSibling(1);
    audioBar.querySelector('[data-ab="close"]').onclick = stopAudio;
    audioBar.querySelector('[data-ab="dl"]').onclick = () => audioTrack && download(audioTrack.name, audioTrack.path);
    const seek = audioBar.querySelector(".ab-seek");
    // 只有用户正在拖动进度条时才不回写进度;松开必须复位,否则进度显示会“停住”
    const startSeek = () => {
      seeking = true;
    };
    const endSeek = () => {
      seeking = false;
    };
    seek.addEventListener("pointerdown", startSeek);
    seek.addEventListener("touchstart", startSeek);
    seek.addEventListener("keydown", startSeek);
    ["pointerup", "pointercancel", "touchend", "touchcancel", "change", "blur", "keyup"].forEach((t) => seek.addEventListener(t, endSeek));
    window.addEventListener("pointerup", endSeek);
    seek.oninput = () => {
      const a = ensureAudioEl();
      if (isFinite(a.duration) && a.duration > 0) a.currentTime = (Number(seek.value) / 1000) * a.duration;
    };
    bindAudioDrag();
  }
  syncAudioBar();
}

/** 卡片可拖动:按住非控件区域拖动,位置限制在窗口内 */
function bindAudioDrag() {
  let drag = null;
  audioBar.addEventListener("pointerdown", (ev) => {
    if (ev.button !== 0) return;
    if (ev.target.closest("button, input, .ab-seek")) return;
    const r = audioBar.getBoundingClientRect();
    drag = { dx: ev.clientX - r.left, dy: ev.clientY - r.top };
    // 入场动画会锁定 transform,拖动前先关掉(否则 translateX(-50%) 会覆盖内联定位)
    audioBar.style.animation = "none";
    audioBar.classList.add("dragging");
    try {
      audioBar.setPointerCapture(ev.pointerId);
    } catch (e) {}
  });
  audioBar.addEventListener("pointermove", (ev) => {
    if (!drag) return;
    const w = audioBar.offsetWidth;
    const h = audioBar.offsetHeight;
    const x = Math.min(Math.max(0, ev.clientX - drag.dx), Math.max(0, window.innerWidth - w));
    const y = Math.min(Math.max(0, ev.clientY - drag.dy), Math.max(0, window.innerHeight - h));
    audioBar.style.transform = "none";
    audioBar.style.left = x + "px";
    audioBar.style.top = y + "px";
    audioBar.style.bottom = "auto";
    audioBar.style.margin = "0";
  });
  const stop = () => {
    if (!drag) return;
    drag = null;
    audioBar.classList.remove("dragging");
  };
  audioBar.addEventListener("pointerup", stop);
  audioBar.addEventListener("pointercancel", stop);
}

function syncAudioBar() {
  if (!audioBar) return;
  const a = ensureAudioEl();
  const playing = !a.paused && !a.ended;
  audioBar.classList.toggle("playing", playing);
  audioBar.querySelector(".ab-play").innerHTML = playing ? ICN.pause : ICN.play;
  if (audioTrack) audioBar.querySelector(".ab-title").textContent = audioTrack.name;
  const dur = isFinite(a.duration) ? a.duration : 0;
  const cur = isFinite(a.currentTime) ? a.currentTime : 0;
  audioBar.querySelector(".ab-time").textContent = `${fmtClock(cur)} / ${fmtClock(dur)}`;
  const multi = audioList.length > 1;
  audioBar.querySelector('[data-ab="prev"]').disabled = !multi;
  audioBar.querySelector('[data-ab="next"]').disabled = !multi;
  const seek = audioBar.querySelector(".ab-seek");
  if (!seeking) seek.value = dur ? String(Math.round((cur / dur) * 1000)) : "0";
}

export function stopAudio() {
  if (audioEl) {
    audioEl.pause();
    audioEl.removeAttribute("src");
    audioEl.load();
  }
  audioTrack = null;
  audioList = [];
  audioIdx = -1;
  seeking = false;
  if (audioBar) {
    audioBar.remove();
    audioBar = null;
    document.body.classList.remove("audio-on");
  }
}

export async function openMedia(name, mask, fullPath) {
  const share = session.share;
  const path = fullPath ? fullPath : childPath(name);
  let meta;
  try {
    meta = await api("GET", `/api/file/meta/${share.id}?path=${encodeURIComponent(path)}`);
  } catch (e) {
    if (mask) mask.closest(".modal-mask").remove();
    return toast(e.message);
  }
  // 图片:整页查看器(可切换全部图片);音频:底部常驻播放器(不挡浏览)
  if (meta.kind === "image") {
    if (mask) mask.closest(".modal-mask").remove();
    openImageViewer(name, path);
    return;
  }
  if (meta.kind === "audio") {
    if (mask) mask.closest(".modal-mask").remove();
    playAudio(share.id, path, name);
    return;
  }
  mask = pvPrepare(mask, name);
  const sEl = mask.querySelector("#pv-size");
  if (sEl) sEl.textContent = `${fmtSize(meta.size)} · ${meta.mime || ""}`;
  mask.querySelector("[data-dl]").onclick = () => download(name, path);
  const sh = mask.querySelector("[data-share]");
  if (sh) sh.onclick = () => import("./dialogs.js").then((m) => m.shareModal(share.id, path, name));
  if (sh) sh.onclick = () => import("./dialogs.js").then((m) => m.shareModal(share.id, path, name));
  const body = mask.querySelector("#pv-body");
  pvCancel();
  const st = await api("GET", `/api/media/status/${share.id}?path=${encodeURIComponent(path)}`);
  const play = (src) => {
    body.innerHTML = `<video controls autoplay src="${esc(src)}"></video>`;
    const v = body.querySelector("video");
    v.style.maxWidth = "100%";
    v.style.maxHeight = "78vh";
    v.style.borderRadius = "10px";
  };
  if ((st.direct || st.state === "ready") && st.src) {
    play(st.src);
    return;
  }
  if (st.state === "unsupported") {
    body.innerHTML = `<p style="padding:30px;color:var(--muted)">服务器未安装 ffmpeg,该格式无法在线播放(仍可下载)</p>`;
    return;
  }
  body.innerHTML = '<div style="display:flex;flex-direction:column;align-items:center;gap:12px"><div class="spinner"></div><span style="color:var(--muted);font-size:13px">首次播放需要转码,请稍候...</span></div>';
  const poll = async () => {
    if (!document.querySelector(".modal.pv")) { pvTimer = null; return; }
    const st2 = await api("GET", `/api/media/status/${share.id}?path=${encodeURIComponent(path)}`).catch(() => null);
    if (st2 && st2.state === "ready" && st2.src) { pvTimer = null; play(st2.src); }
    else if (st2 && st2.state === "converting") pvTimer = setTimeout(poll, 2500);
    else { pvTimer = null; body.innerHTML = `<p style="padding:30px;color:var(--danger)">转码失败,无法播放该文件(可下载)</p>`; }
  };
  pvTimer = setTimeout(poll, 2500);
}

export async function openFile(name, mask, fullPath) {
  const share = session.share;
  const path = fullPath ? fullPath : childPath(name);
  let meta;
  try {
    meta = await api("GET", `/api/file/meta/${share.id}?path=${encodeURIComponent(path)}`);
  } catch (e) {
    if (mask) mask.remove();
    return toast(e.message);
  }
  mask = pvPrepare(mask, name);
  const sEl = mask.querySelector("#pv-size");
  if (sEl) sEl.textContent = `${fmtSize(meta.size)} · ${meta.mime || ""}`;
  mask.querySelector("[data-dl]").onclick = () => download(name, path);
  const sh = mask.querySelector("[data-share]");
  if (sh) sh.onclick = () => import("./dialogs.js").then((m) => m.shareModal(share.id, path, name));
  const body = mask.querySelector("#pv-body");
  pvCancel();
  pvCancel();
  const ext = meta.ext;
  const TEXT_EXTS = ["txt", "log", "md", "markdown", "yaml", "yml", "conf", "ini", "toml", "json", "xml", "html", "htm", "css", "js", "mjs", "ts", "sh", "bash", "py", "sql", "properties", "env"];
  if (ext === "pdf") {
    body.classList.add("pv-fill");
    body.innerHTML = `<iframe class="pv-pdf" src="${esc(mediaUrl(share.id, path))}" title="PDF 预览"></iframe>`;
    return;
  }
  if (TEXT_EXTS.includes(ext)) return pvText(share, path, body, meta.writable, ext);
  if (["xlsx", "xlsm", "xls", "ods", "csv", "tsv"].includes(ext)) return pvSheet(share, path, body, ext, meta.writable);
  if (["docx", "docm"].includes(ext)) return pvDocx(share, path, body);
  if (ext === "pptx") return pvPptx(share, path, body);
  if (["doc", "ppt", "odt", "odp", "rtf"].includes(ext)) {
    body.innerHTML = `<div class="pv-dl-only">${ext === "doc" || ext === "ppt" ? "旧版 Office 格式暂不支持在线预览" : "该格式暂不支持在线预览"},请下载后查看</div>`;
    return;
  }
  body.innerHTML = '<div class="pv-dl-only">该类型文件请下载后查看</div>';
}

/** 拉取原始字节并智能解码:UTF-8(BOM) > UTF-8 严格校验 > GBK 兜底 */
async function fetchTextSmart(shareId, path) {
  const resp = await fetch(mediaUrl(shareId, path));
  if (!resp.ok) throw new Error("读取失败 (" + resp.status + ")");
  const buf = await resp.arrayBuffer();
  const bytes = new Uint8Array(buf);
  if (bytes[0] === 0xff && bytes[1] === 0xfe) return { text: new TextDecoder("utf-16le").decode(buf), enc: "utf-16" };
  if (bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) return { text: new TextDecoder("utf-8").decode(buf.subarray(3)), enc: "utf-8" };
  try {
    return { text: new TextDecoder("utf-8", { fatal: true }).decode(buf), enc: "utf-8" };
  } catch (e) {
    try { return { text: new TextDecoder("gbk").decode(buf), enc: "gbk" }; } catch (e2) {}
    return { text: new TextDecoder("utf-8").decode(buf), enc: "utf-8" };
  }
}

async function pvText(share, path, body, writable) {
  const editable = !!session.user && !!writable;
  let r;
  try {
    // 直接取原始字节解码:服务端文本接口按 UTF-8 解,GBK 等本地编码会乱码
    r = await fetchTextSmart(share.id, path);
  } catch (e) {
    body.innerHTML = `<p class="pv-msg">${esc(e.message)}</p>`;
    return;
  }
  body.classList.add("pv-fill");
  const ta = document.createElement("textarea");
  ta.className = "pv-textarea" + (editable ? " editable" : "");
  ta.value = r.text;
  ta.readOnly = !editable;
  ta.spellcheck = false;
  body.appendChild(ta);
  if (r.enc !== "utf-8" && editable) {
    const hint = document.createElement("div");
    hint.className = "pv-sheet-hint";
    hint.textContent = "检测到非 UTF-8 编码(" + (r.enc === "gbk" ? "GBK/ANSI" : r.enc) + "),内容已正确显示;保存后将转为 UTF-8。";
    body.insertBefore(hint, body.firstChild);
  }
  if (!editable) return;
  const bar = document.createElement("div");
  bar.className = "pv-edit-bar";
  bar.innerHTML = `<button class="btn mini" id="pv-save">${ICN.save}保存 (Ctrl+S)</button><span id="pv-save-msg" class="pv-save-msg"></span>`;
  body.insertBefore(bar, body.firstChild);
  const saveBtn = bar.querySelector("#pv-save");
  const msgEl = bar.querySelector("#pv-save-msg");
  const doSave = async () => {
    saveBtn.disabled = true;
    msgEl.textContent = "保存中...";
    try {
      const rr = await api("POST", `/api/file/text/${share.id}?path=${encodeURIComponent(path)}`, { content: ta.value });
      msgEl.textContent = `✓ 已保存 (${fmtSize(rr.size)})` + (r.enc !== "utf-8" ? "(已转 UTF-8)" : "");
      r.enc = "utf-8";
      markSaved();
    } catch (e) {
      msgEl.textContent = "保存失败: " + e.message;
    }
    setTimeout(() => { saveBtn.disabled = false; }, 1800);
  };
  saveBtn.onclick = doSave;
  ta.addEventListener("keydown", (ev) => {
    if ((ev.ctrlKey || ev.metaKey) && ev.key === "s") {
      ev.preventDefault();
      doSave();
    }
  });
}

function markSaved() {
  const mask = document.querySelector(".modal.pv");
  if (!mask) return;
  const dl = mask.querySelector("[data-dl]");
  if (dl) dl.innerHTML = ICN.download + "已更新,重新下载";
}

async function pvDocx(share, path, body) {
  if (typeof docx === "undefined" || !docx.renderAsync) {
    body.innerHTML = '<p class="pv-msg">文档渲染组件未加载</p>';
    return;
  }
  body.classList.add("pv-fill");
  body.innerHTML = '<div class="pv-loading"><div class="spinner"></div><span>文档加载中...</span></div>';
  try {
    const resp = await fetch(mediaUrl(share.id, path));
    if (!resp.ok) throw new Error("读取失败 (" + resp.status + ")");
    const blob = await resp.blob();
    body.innerHTML = "";
    const holder = document.createElement("div");
    holder.className = "pv-docx";
    body.appendChild(holder);
    await docx.renderAsync(blob, holder, null, { inWrapper: true, ignoreLastRenderedPageBreak: true });
  } catch (e) {
    if (body.isConnected) body.innerHTML = `<p class="pv-msg">文档无法解析: ${esc(e.message)}</p>`;
  }
}

/** PPTX 大纲视图:解包幻灯片 XML,按页展示文字 */
async function pvPptx(share, path, body) {
  if (typeof JSZip === "undefined") {
    body.innerHTML = '<p class="pv-msg">预览组件未加载</p>';
    return;
  }
  body.classList.add("pv-fill");
  body.innerHTML = '<div class="pv-loading"><div class="spinner"></div><span>演示文稿加载中...</span></div>';
  try {
    const resp = await fetch(mediaUrl(share.id, path));
    if (!resp.ok) throw new Error("读取失败 (" + resp.status + ")");
    const zip = await JSZip.loadAsync(await resp.arrayBuffer());
    const slides = Object.keys(zip.files)
      .filter((n) => /^ppt\/slides\/slide\d+\.xml$/.test(n))
      .sort((a, b) => parseInt(a.match(/(\d+)/)[1], 10) - parseInt(b.match(/(\d+)/)[1], 10));
    if (!slides.length) throw new Error("没有找到幻灯片");
    body.innerHTML = "";
    const hint = document.createElement("div");
    hint.className = "pv-sheet-hint";
    hint.textContent = `大纲视图:共 ${slides.length} 页幻灯片的文字内容。`;
    body.appendChild(hint);
    for (const sn of slides) {
      const xml = await zip.file(sn).async("string");
      const dom = new DOMParser().parseFromString(xml, "application/xml");
      if (dom.querySelector("parsererror")) throw new Error("幻灯片解析失败");
      const texts = [...dom.getElementsByTagName("a:t")].map((t) => t.textContent.trim()).filter(Boolean);
      const card = document.createElement("div");
      card.className = "pv-slide";
      card.innerHTML =
        `<div class="pv-slide-no">第 ${parseInt(sn.match(/(\d+)/)[1], 10)} 页</div>` +
        (texts.length ? texts.map((t) => `<p>${esc(t)}</p>`).join("") : '<p class="pv-slide-empty">(本页无可显示的文本)</p>');
      body.appendChild(card);
    }
  } catch (e) {
    if (body.isConnected) body.innerHTML = `<p class="pv-msg">演示文稿无法解析: ${esc(e.message)}</p>`;
  }
}

// ---------------- 表格(渲染保留格式与合并单元格,保存不破坏原表) ----------------

const SHEET_MAX_ROWS = 1000;
const SHEET_MAX_COLS = 100;
let sheetWb = null;
let sheetName = "";
let sheetPath = "";
let sheetExt = "";
let sheetEditable = false;
let sheetRaw = null;
let sheetEnc = "utf-8";
let sheetTruncated = false;

async function pvSheet(share, path, body, ext, writable) {
  if (typeof XLSX === "undefined") {
    body.innerHTML = '<p class="pv-msg">表格解析组件未加载</p>';
    return;
  }
  sheetPath = path;
  sheetExt = ext;
  sheetRaw = null;
  sheetTruncated = false;
  sheetEnc = "utf-8";
  sheetEditable = !!session.user && !!writable;
  body.classList.add("pv-fill");
  body.innerHTML = '<div class="pv-loading"><div class="spinner"></div><span>表格加载中...</span></div>';
  try {
    if (ext === "csv" || ext === "tsv") {
      const r = await fetchTextSmart(share.id, path);
      sheetEnc = r.enc;
      sheetWb = XLSX.read(r.text, { type: "string" });
    } else {
      const resp = await fetch(mediaUrl(share.id, path));
      if (!resp.ok) throw new Error("读取失败 (" + resp.status + ")");
      sheetWb = XLSX.read(await resp.arrayBuffer(), { cellNF: true });
    }
  } catch (e) {
    if (body.isConnected) body.innerHTML = `<p class="pv-msg">${esc(e.message)}</p>`;
    return;
  }
  const sheets = sheetWb.SheetNames;
  if (!sheets.length) {
    body.innerHTML = '<p class="pv-msg">表格中没有工作表</p>';
    return;
  }
  const tabbar = document.createElement("div");
  tabbar.className = "pv-sheet-tabs";
  let gridBox = null;
  sheets.forEach((sn, i) => {
    const b = document.createElement("button");
    b.className = "btn mini sheet-tab" + (i === 0 ? " active" : "");
    b.textContent = sn;
    b.onclick = () => {
      tabbar.querySelectorAll(".sheet-tab").forEach((x) => x.classList.remove("active"));
      b.classList.add("active");
      renderGrid(gridBox, sn);
    };
    tabbar.appendChild(b);
  });
  body.innerHTML = "";
  body.appendChild(tabbar);
  gridBox = document.createElement("div");
  gridBox.className = "pv-grid-wrap";
  body.appendChild(gridBox);
  renderGrid(gridBox, sheets[0]);
}

function colLabel(i) {
  let s = "";
  i += 1;
  while (i > 0) {
    const m = (i - 1) % 26;
    s = String.fromCharCode(65 + m) + s;
    i = Math.floor((i - 1) / 26);
  }
  return s;
}

function renderGrid(box, name) {
  if (!box) return;
  sheetName = name;
  const ws = sheetWb.Sheets[name];
  if (!ws) return;
  const range = ws["!ref"] || "A1";
  const info = XLSX.utils.decode_range(range);
  const totalRows = info.e.r + 1;
  const totalCols = info.e.c + 1;
  const rows = Math.min(totalRows, SHEET_MAX_ROWS);
  const cols = Math.min(totalCols, SHEET_MAX_COLS);
  sheetTruncated = rows < totalRows || cols < totalCols;
  // 两份矩阵:raw 保存用,formatted 显示用(日期 / 百分比 / 货币按单元格格式渲染)
  const rawAoa = XLSX.utils.sheet_to_json(ws, { header: 1, raw: true, defval: "" });
  const fmtAoa = XLSX.utils.sheet_to_json(ws, { header: 1, raw: false, defval: "" });
  sheetRaw = rawAoa;
  const at = (m, r, c) => (Array.isArray(m[r]) && m[r][c] !== undefined && m[r][c] !== null ? m[r][c] : "");
  const covered = new Set();
  const spans = new Map();
  (ws["!merges"] || []).forEach((m) => {
    if (m.s.r >= rows || m.s.c >= cols) return;
    const er = Math.min(m.e.r, rows - 1), ec = Math.min(m.e.c, cols - 1);
    for (let r = m.s.r; r <= er; r++)
      for (let c = m.s.c; c <= ec; c++) {
        if (r === m.s.r && c === m.s.c) continue;
        covered.add(r + "," + c);
      }
    spans.set(m.s.r + "," + m.s.c, { rs: er - m.s.r + 1, cs: ec - m.s.c + 1 });
  });
  const tab = document.createElement("table");
  tab.className = "pv-grid";
  const th = document.createElement("tr");
  th.innerHTML = `<th class="corner"></th>` + Array.from({ length: cols }, (_, i) => `<th class="colhead">${colLabel(i)}</th>`).join("");
  tab.appendChild(th);
  const colW = Array.isArray(ws["!cols"]) ? ws["!cols"] : [];
  const cg = document.createElement("colgroup");
  for (let c = 0; c < cols; c++) {
    const w = colW[c] && (colW[c].wch || colW[c].width);
    const px = Math.max(72, Math.min(360, w ? Math.round(w * 8 + 16) : 96));
    const col = document.createElement("col");
    col.style.width = px + "px";
    cg.appendChild(col);
  }
  tab.appendChild(cg);
  const editableNow = sheetEditable && !sheetTruncated;
  for (let r = 0; r < rows; r++) {
    const tr = document.createElement("tr");
    tr.innerHTML = `<td class="rowhead">${r + 1}</td>`;
    for (let c = 0; c < cols; c++) {
      if (covered.has(r + "," + c)) continue;
      const td = document.createElement("td");
      const disp = String(at(fmtAoa, r, c));
      const raw = at(rawAoa, r, c);
      td.textContent = disp;
      td.dataset.r = r;
      td.dataset.c = c;
      td.dataset.raw = typeof raw === "object" && raw !== null ? JSON.stringify(raw) : String(raw);
      td.title = disp;
      if (typeof raw === "number") td.classList.add("num");
      if (spans.has(r + "," + c)) {
        const s = spans.get(r + "," + c);
        if (s.rs > 1) td.rowSpan = s.rs;
        if (s.cs > 1) td.colSpan = s.cs;
      }
      if (editableNow) {
        td.contentEditable = "true";
        td.addEventListener("input", () => {
          td.dataset.edited = "1";
        });
      }
      tr.appendChild(td);
    }
    tab.appendChild(tr);
  }
  box.innerHTML = "";
  box.appendChild(tab);
  if (sheetTruncated) {
    const hint = document.createElement("div");
    hint.className = "pv-sheet-hint";
    hint.textContent = `表格较大:共 ${totalRows} 行 × ${totalCols} 列,当前仅显示前 ${rows} 行 × ${cols} 列(只读)。请下载后处理完整内容。`;
    box.insertBefore(hint, box.firstChild);
    return;
  }
  if (!sheetEditable) return;
  const bar = document.createElement("div");
  bar.className = "pv-edit-bar";
  bar.innerHTML = `<button class="btn mini" id="pv-save">${ICN.save}保存表格</button><span id="pv-save-msg" class="pv-save-msg"></span>`;
  box.insertBefore(bar, box.firstChild);
  const saveBtn = bar.querySelector("#pv-save");
  const msgEl = bar.querySelector("#pv-save-msg");
  saveBtn.onclick = async () => {
    saveBtn.disabled = true;
    msgEl.textContent = "保存中...";
    try {
      if (sheetExt === "csv" || sheetExt === "tsv") {
        // 文本类表格:整表重建文本(无格式概念),走文本端点保存(UTF-8)
        const out = [];
        for (let r = 0; r < rows; r++) {
          const row = [];
          for (let c = 0; c < cols; c++) row.push(at(rawAoa, r, c));
          out.push(row);
        }
        tab.querySelectorAll("td[data-r]").forEach((td) => {
          if (td.dataset.edited === undefined) return;
          const r = +td.dataset.r, c = +td.dataset.c;
          if (out[r] && c < out[r].length) out[r][c] = td.textContent.trim();
        });
        const csv = XLSX.utils.sheet_to_csv(XLSX.utils.aoa_to_sheet(out), { FS: sheetExt === "csv" ? "," : "\t" });
        await api("POST", `/api/file/text/${session.share.id}?path=${encodeURIComponent(sheetPath)}`, { content: csv });
        msgEl.textContent = "✓ 已保存" + (sheetEnc === "gbk" ? "(已转为 UTF-8)" : "");
      } else {
        // 在原工作表对象上只更新编辑过的单元格:格式 / 列宽 / 合并 / 其它工作表全部保留
        let changed = 0;
        tab.querySelectorAll("td[data-r]").forEach((td) => {
          if (td.dataset.edited === undefined) return;
          const addr = XLSX.utils.encode_cell({ r: +td.dataset.r, c: +td.dataset.c });
          const text = td.textContent.trim();
          sheetWb.Sheets[name][addr] = /^-?\d+(\.\d+)?$/.test(text)
            ? { t: "n", v: parseFloat(text) }
            : { t: "s", v: text };
          changed++;
        });
        if (!changed) {
          msgEl.textContent = "没有修改";
        } else {
          const bytes = XLSX.write(sheetWb, { bookType: sheetExt === "xls" ? "biff8" : sheetExt === "ods" ? "ods" : sheetExt === "xlsm" ? "xlsm" : "xlsx", type: "array" });
          const headers = {};
          if (session.token) headers["Authorization"] = "Bearer " + session.token;
          headers["Content-Type"] = "application/octet-stream";
          const resp = await fetch(`/api/file/binary/${session.share.id}?path=${encodeURIComponent(sheetPath)}`, { method: "POST", headers, body: bytes });
          const j = (resp.headers.get("content-type") || "").includes("json") ? await resp.json().catch(() => null) : null;
          if (!resp.ok) throw new Error(j && j.error ? j.error : "保存失败 (" + resp.status + ")");
          msgEl.textContent = "✓ 已保存";
        }
      }
      markSaved();
    } catch (e) {
      msgEl.textContent = "保存失败: " + e.message;
    }
    setTimeout(() => { saveBtn.disabled = false; }, 1800);
  };
}

// 预览键盘导航
document.addEventListener("keydown", (ev) => {
  const pv = document.querySelector(".modal.pv");
  if (!pv) return;
  if (ev.key === "Escape") return pv.closest(".modal-mask").remove();
  if (ev.target.closest("input, select, textarea, [contenteditable]")) return;
  if (ev.key === "ArrowLeft") return pvNav(-1);
  if (ev.key === "ArrowRight") return pvNav(1);
});
