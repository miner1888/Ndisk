// 分片断点上传 + 右下角任务面板。
import { api, putChunk, putDirect } from "../core/api.js";
import { session, esc, fmtSize, MOVE_MIME } from "../core/state.js";
import { ICN } from "../core/icons.js";
import { toast } from "../core/ui.js";
import { loadFiles } from "./browser.js";

export function pickFiles(ev) {
  const files = Array.from(ev.target.files || []);
  if (!files.length) return;
  if (!session.share) return toast("没有可上传的空间");
  const dest = { share: session.share, path: session.path };
  showPanel();
  (async () => {
    for (const f of files) {
      await uploadOne(f, dest);
    }
    loadFiles();
  })();
}

function showPanel() {
  const p = document.getElementById("uploadPanel");
  p.classList.remove("hidden");
  if (p.dataset.bound !== "1") {
    p.dataset.bound = "1";
    p.innerHTML = `
      <div class="up-head">
        <span class="up-title">${ICN.upload}上传任务 <span id="up-count" class="up-count-badge">0</span></span>
        <button class="btn mini ghost" id="up-clear" title="清除已完成">清除已完成</button>
        <button class="btn mini ghost" id="up-close" title="关闭" style="margin-left:auto">${ICN.close}</button>
      </div>
      <div class="up-list"></div>`;
    document.getElementById("up-close").onclick = () => p.classList.add("hidden");
    document.getElementById("up-clear").onclick = () => {
      const list = p.querySelector(".up-list");
      list.querySelectorAll(".up-item.done").forEach((n) => n.remove());
      p.querySelector("#up-count").textContent = list.querySelectorAll(".up-item").length;
      if (!list.querySelector(".up-item")) p.classList.add("hidden");
    };
  }
}

function updateCount() {
  const el = document.getElementById("up-count");
  if (!el) return;
  const list = document.querySelector("#uploadPanel .up-list");
  el.textContent = list ? list.querySelectorAll(".up-item").length : 0;
}

/** 一行上传任务的 DOM 与进度控制句柄。 */
function uploadRow(name, size) {
  const panel = document.getElementById("uploadPanel");
  const list = panel.querySelector(".up-list");
  const item = document.createElement("div");
  item.className = "up-item";
  item.innerHTML = `
    <span class="up-name" title="${esc(name)}">${esc(name)}</span>
    <div class="up-bar"><div class="up-fill"></div></div>
    <span class="up-pct">0%</span>
    <span class="up-state"></span>
    <button class="btn mini ghost up-retry hidden">重试</button>`;
  list.appendChild(item);
  updateCount();
  const fill = item.querySelector(".up-fill");
  const pct = item.querySelector(".up-pct");
  const state = item.querySelector(".up-state");
  const total = size || 1;
  const set = (done) => {
    const p = Math.min(100, Math.round((done / total) * 100));
    fill.style.width = p + "%";
    pct.textContent = p + "%";
  };
  return {
    setProgress: (done) => set(done || 0),
    setText: (txt) => {
      pct.textContent = txt;
      item.title = txt;
    },
    setTextSize: (done, tot) => {
      pct.textContent = Math.round(done) + " / " + fmtSize(tot);
      item.title = "";
    },
    markDone: () => {
      set(100);
      item.classList.remove("fail");
      item.classList.add("done");
      state.innerHTML = ICN.check;
      item.querySelector(".up-retry").classList.add("hidden");
    },
    markFail: (msg) => {
      item.classList.remove("done");
      item.classList.add("fail");
      pct.textContent = "失败";
      item.title = msg || "上传失败";
      state.innerHTML = ICN.close;
      item.querySelector(".up-retry").classList.remove("hidden");
      const p = document.getElementById("uploadPanel");
      if (p.classList.contains("hidden")) {
        p.classList.remove("hidden");
        toast("上传失败:" + item.querySelector(".up-name").textContent);
      }
    },
    markResume: () => {
      item.classList.remove("fail", "done");
      item.title = "";
      state.innerHTML = "";
      item.querySelector(".up-retry").classList.add("hidden");
    },
    showRetry: (cb) => {
      const btn = item.querySelector(".up-retry");
      btn.onclick = cb;
      btn.classList.remove("hidden");
    },
  };
}

async function uploadOne(file, dest) {
  const row = uploadRow(file.name, file.size);
  const directMax = session.directMax || 16 * 1024 * 1024;
  if (file.size <= directMax) {
    row.setText("直传中");
    const r = await putDirect(dest, file, (loaded) => row.setProgress(loaded, file.size));
    if (r.ok) {
      row.markDone();
      row.setTextSize(file.size, file.size);
      return;
    }
    // 直传失败回退到分片流程。
    row.markResume();
  }
  row.setText("准备中");
  let init;
  try {
    init = await api("POST", "/api/upload/init", {
      share_id: dest.share.id,
      path: dest.path,
      filename: file.name,
      size: file.size,
    });
  } catch (e) {
    row.markFail(e.message || "无法开始上传");
    toast("上传失败:" + file.name + "(" + (e.message || "未知错误") + ")");
    return;
  }
  const { token, chunk_size, num_parts } = init;
  const uploaded = await uploadParts(token, chunk_size, num_parts, file, row);
  if (!uploaded) {
    row.markFail("网络中断,可稍后再试");
    toast("上传失败:" + file.name + "(网络中断,可稍后重试)");
    row.showRetry(async () => {
      row.markResume();
      await resumeSession(token, chunk_size, num_parts, file, row);
    });
    return;
  }
  await finish(token, chunk_size, num_parts, file, row);
}

async function resumeSession(token, chunk_size, num_parts, file, row) {
  const uploaded = await uploadParts(token, chunk_size, num_parts, file, row);
  if (!uploaded) {
    row.markFail("网络中断,可稍后再试");
    toast("上传失败:" + file.name + "(网络中断,可稍后重试)");
    return;
  }
  await finish(token, chunk_size, num_parts, file, row);
}

async function uploadParts(token, chunk_size, num_parts, file, row) {
  // 先问服务端已收分片,跳过它们(断点续传)。
  let doneParts = [];
  try {
    const p = await api("GET", `/api/upload/parts/${token}`);
    doneParts = p.parts || [];
    if (doneParts.length) row.setText("断点续传...");
  } catch (e) {}
  for (let i = 0; i < num_parts; i++) {
    if (doneParts.indexOf(i) >= 0) continue;
    const blob = file.slice(i * chunk_size, Math.min(file.size, (i + 1) * chunk_size));
    let ok = false;
    for (let attempt = 1; attempt <= 3 && !ok; attempt++) {
      ok = await putChunk(token, i, blob, (loaded) => {
        row.setProgress(i * chunk_size + loaded, file.size);
      });
      if (!ok && attempt < 3) {
        row.setText(`第 ${i + 1}/${num_parts} 片重试 ${attempt}/3`);
        await new Promise((r) => setTimeout(r, 1000 * attempt));
      }
    }
    if (!ok) return false;
  }
  return true;
}

async function finish(token, chunk_size, num_parts, file, row) {
  try {
    await api("POST", `/api/upload/complete/${token}`);
    row.markDone();
    row.setTextSize(file.size, file.size);
  } catch (e) {
    row.markFail("合并失败: " + (e.message || "未知错误"));
    toast("上传失败:" + file.name + "(" + (e.message || "未知错误") + ")");
    row.showRetry(async () => {
      row.markResume();
      await resumeSession(token, chunk_size, num_parts, file, row);
    });
  }
}

/** 拖拽外部文件到文件区即上传(内部移动由 browser.js 处理)。 */
export function bindDropUpload() {
  const el = document.getElementById("files");
  if (!el) return;
  const isInternalMove = (ev) => ev.dataTransfer && Array.from(ev.dataTransfer.types || []).includes(MOVE_MIME);
  el.addEventListener("dragover", (ev) => {
    if (isInternalMove(ev)) return;
    ev.preventDefault();
    el.classList.add("drag-over");
  });
  el.addEventListener("dragleave", (ev) => {
    if (isInternalMove(ev)) return;
    el.classList.remove("drag-over");
  });
  el.addEventListener("drop", (ev) => {
    if (isInternalMove(ev)) return;
    ev.preventDefault();
    el.classList.remove("drag-over");
    const files = ev.dataTransfer && ev.dataTransfer.files;
    if (!files || !files.length) return;
    if (!session.can || !session.can.upload) return toast("当前没有上传权限");
    pickFiles({ target: { files } });
  });
}
