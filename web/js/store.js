// 全局会话与视图状态(模块间共享的单例)
// 内部拖拽移动的自定义 dataTransfer 类型(与外部文件上传区分)
export const MOVE_MIME = "application/x-ndisk-move";

export const session = {
  token: localStorage.getItem("nd_token") || "",
  user: null,
  // 服务端下发的直传大小阈值:小于等于该值的文件走单请求直传
  directMax: 16 * 1024 * 1024,
  shares: [],
  share: null,
  path: "",
  can: {},
  view: localStorage.getItem("nd_view") || "grid",
  sort: localStorage.getItem("nd_sort") || "name",
  filter: "",
  searchOpen: false,
  albumView: false,
  albumImages: [],
  lastEntries: [],
  sel: new Set(),
  selDir: new Set(),

  save() {
    if (this.token) localStorage.setItem("nd_token", this.token);
    else localStorage.removeItem("nd_token");
  },
  expire() {
    this.token = "";
    this.user = null;
    this.shares = [];
    this.share = null;
    this.save();
    import("./main.js").then((m) => m.showLogin());
  },
  reset() {
    this.token = "";
    this.user = null;
    this.shares = [];
    this.share = null;
    this.path = "";
    this.can = {};
    this.searchOpen = false;
    this.sel.clear();
    this.selDir.clear();
    this.albumView = false;
    this.save();
  },
};

export function esc(s) {
  return String(s == null ? "" : s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

export function fmtSize(n) {
  if (n == null) return "";
  if (n < 1024) return n + " B";
  const u = ["KB", "MB", "GB", "TB"];
  let v = n, i = -1;
  do { v /= 1024; i++; } while (v >= 1024 && i < u.length - 1);
  return v.toFixed(1) + " " + u[i];
}

export function fmtTime(ts) {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  const p = (n) => String(n).padStart(2, "0");
  const now = new Date();
  if (d.toDateString() === now.toDateString()) return `今天 ${p(d.getHours())}:${p(d.getMinutes())}`;
  const yest = new Date(now.getTime() - 86400000);
  if (d.toDateString() === yest.toDateString()) return `昨天 ${p(d.getHours())}:${p(d.getMinutes())}`;
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function fmtTimeFull(ts) {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}/${p(d.getMonth() + 1)}/${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function childPath(name) {
  return session.path ? session.path + "/" + name : name;
}

export function mediaUrl(shareId, path, extra) {
  let u = `/api/media/${shareId}?path=${encodeURIComponent(path)}`;
  if (extra) u += "&" + extra;
  return u;
}
