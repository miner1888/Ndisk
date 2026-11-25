// 相册视图:只展示个人空间内的全部图片与视频缩略图,不显示文件名等元信息。
import { api } from "../core/api.js";
import { session, esc } from "../core/state.js";
import { ICN } from "../core/icons.js";
import { openMedia } from "./preview.js";
import { loadFiles } from "./browser.js";
import { detailModal } from "./dialogs.js";
import { openMenuAt } from "../core/ui.js";

let albumSeq = 0;

export async function loadAlbum() {
  const el = document.getElementById("files");
  const share = session.share;
  if (!share) return;
  session.albumView = true;
  session.path = "";
  session.filter = "";
  session.sel.clear();
  session.selDir.clear();
  session.albumImages = [];
  session.can = {};
  const seq = ++albumSeq;
  renderAlbumToolbar(0);
  el.innerHTML = '<div class="center"><div class="spinner"></div></div>';
  let r;
  try {
    r = await api("GET", `/api/album/${share.id}`);
  } catch (e) {
    if (seq !== albumSeq) return;
    el.innerHTML = `<div class="center" style="color:var(--danger)">${esc(e.message)}</div>`;
    return;
  }
  if (seq !== albumSeq || !session.albumView) return;
  renderAlbumToolbar(r.count || 0);
  renderAlbumGrid(r.images || []);
  import("./browser.js").then((m) => m.renderFab());
}

function renderAlbumToolbar(count) {
  const el = document.getElementById("toolbar");
  if (!el) return;
  el.innerHTML = `
    <button class="btn mini ghost" id="btn-album-back" title="返回文件">${ICN.back}返回</button>
    <div class="crumb"><span class="album-title">${ICN.album}<b>相册</b></span>
      <span style="color:var(--muted);font-size:13px">${count ? count + " 项" : ""}</span></div>
    <div class="spacer"></div>`;
  const bk = document.getElementById("btn-album-back");
  if (bk) bk.onclick = () => {
    session.path = session.albumReturnPath || "";
    loadFiles();
  };
}

function renderAlbumGrid(images) {
  const el = document.getElementById("files");
  session.albumImages = images;
  if (!images.length) {
    el.innerHTML = `<div class="empty"><span class="empty-ico">${ICN.album}</span><div>个人空间里还没有图片或视频</div></div>`;
    return;
  }
  const sid = session.share.id;
  el.innerHTML = `<div class="album-grid">${images
    .map((img) => {
      const p = encodeURIComponent(img.path);
      const isVid = img.kind === "video";
      const thumb = isVid ? `/api/media/thumb/${sid}?path=${p}` : `/api/media/imgthumb/${sid}?path=${p}`;
      const full = `/api/media/${sid}?path=${p}`;
      return `<div class="photo" data-path="${esc(img.path)}" data-name="${esc(img.name)}">
        <img src="${thumb}" loading="lazy" alt="" onerror="this.onerror=null;this.src='${esc(full)}'">
        ${isVid ? `<span class="vbadge">${ICN.play}</span>` : ""}
        <button class="photo-info" type="button" title="详情">${ICN.info}</button>
      </div>`;
    })
    .join("")}</div>`;
  el.querySelectorAll(".photo").forEach((ph) => {
    const path = ph.dataset.path;
    const name = ph.dataset.name;
    ph.onclick = () => openMedia(name, null, path);
    const info = ph.querySelector(".photo-info");
    info.onclick = (ev) => {
      ev.stopPropagation();
      detailModal(name, false, path);
    };
    ph.oncontextmenu = (ev) => {
      ev.preventDefault();
      openMenuAt(ev, [
        { icon: ICN.info, label: "详情", act: () => detailModal(name, false, path) },
        { icon: ICN.play, label: "查看", act: () => openMedia(name, null, path) },
      ]);
    };
  });
}
