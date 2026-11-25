// 侧边栏:高亮由 activeEntry() 唯一判定,导航后调 syncSidebar()。
import { session, esc } from "../core/state.js";
import { ICN } from "../core/icons.js";

const accessName = (a) =>
  a === "guest" ? "游客可访问" : a === "admin" ? "仅管理员" : a === "private" ? "私有" : "登录可访问";

function activeEntry() {
  if (session.albumView) return "album";
  const home = session.shares.find((s) => s.kind === "home" && s.mine);
  const inHome = !!(session.share && home && session.share.id === home.id);
  if (inHome && (session.path === "下载" || session.path.startsWith("下载/"))) return "download";
  if (session.share) return `share:${session.share.id}`;
  return null;
}

async function openDownload() {
  const home = session.shares.find((s) => s.kind === "home" && s.mine);
  if (!home) return;
  session.share = home;
  session.albumView = false;
  session.path = "下载";
  session.filter = "";
  try {
    const { api } = await import("../core/api.js");
    await api("POST", "/api/mkdir", { share_id: home.id, path: "", name: "下载" });
  } catch {
    /* 目录已存在等冲突:忽略 */
  }
  const browser = await import("./browser.js");
  await browser.loadFiles();
  syncSidebar();
  browser.closeSidebar();
}

async function openAlbum() {
  const home = session.shares.find((s) => s.kind === "home" && s.mine);
  if (!home) return;
  session.share = home;
  session.albumReturnPath = session.path;
  session.albumView = true;
  const album = await import("./album.js");
  await album.loadAlbum();
  syncSidebar();
  const browser = await import("./browser.js");
  browser.closeSidebar();
}

async function openShare(id) {
  const s = session.shares.find((x) => x.id == id);
  if (!s) return;
  session.share = s;
  session.albumView = false;
  session.path = "";
  session.filter = "";
  const browser = await import("./browser.js");
  await browser.loadFiles();
  syncSidebar();
  browser.closeSidebar();
}

export function renderSidebar() {
  const el = document.getElementById("shareList");
  if (!el) return;
  if (!session.shares.length) {
    el.innerHTML = '<div style="padding:14px;color:var(--muted);font-size:13px">无可用空间</div>';
    return;
  }
  const active = activeEntry();
  const mineHome = session.shares.filter((s) => s.kind === "home" && s.mine);
  const otherHome = session.shares.filter((s) => s.kind === "home" && !s.mine);
  const shared = session.shares.filter((s) => s.kind !== "home");

  const item = (key, cls, ico, label, badge, dataset) => `
    <div class="share-item ${cls} ${active === key ? "active" : ""}" ${dataset}>
      <span class="ico">${ico}</span>
      <span>${esc(label)}</span>
      ${badge ? `<span class="share-badge">${badge}</span>` : ""}
    </div>`;

  const shareItem = (s) =>
    item(`share:${s.id}`, "", s.kind === "home" ? ICN.home : ICN.folder, s.name, accessName(s.access), `data-share="${s.id}"`);
  const extras = mineHome.length
    ? item("album", "album-item", ICN.album, "相册", "", 'data-album="1"') +
      item("download", "dl-item", ICN.download, "下载", "", 'data-dl="1"')
    : "";

  const section = (title, list) => (list.length ? `<div class="side-sub">${title}</div>${list.map(shareItem).join("")}` : "");

  el.innerHTML = [
    section("我的空间", mineHome) + extras,
    section("用户空间", otherHome),
    section("共享空间", shared),
  ].join("");

  el.querySelector(".share-item[data-dl]")?.addEventListener("click", openDownload);
  el.querySelector(".share-item[data-album]")?.addEventListener("click", openAlbum);
  el.querySelectorAll(".share-item[data-share]").forEach((node) => {
    node.addEventListener("click", () => openShare(node.dataset.share));
  });
}

export function syncSidebar() {
  renderSidebar();
}
