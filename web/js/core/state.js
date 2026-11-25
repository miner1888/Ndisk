// 全局状态:前端唯一的状态单例 + 格式化 / 路径工具。
// 拖拽移动用的自定义 MIME(内部移动与外部拖放上传的区分依据)。
export const MOVE_MIME = "application/x-ndisk-move";

const TOKEN_KEY = "nd_token";
const VIEW_KEY = "nd_view";
const SORT_KEY = "nd_sort";

export const session = {
  token: localStorage.getItem(TOKEN_KEY) || "",
  user: null,
  /** 直传上限(服务端 /api/shares 返回)。 */
  directMax: 16 * 1024 * 1024,
  shares: [],
  share: null,
  path: "",
  can: {},
  view: localStorage.getItem(VIEW_KEY) || "grid",
  sort: localStorage.getItem(SORT_KEY) || "name",
  filter: "",
  searchOpen: false,
  albumView: false,
  albumImages: [],
  albumReturnPath: "",
  lastEntries: [],
  shown: [],
  sel: new Set(),
  selDir: new Set(),

  save() {
    if (this.token) localStorage.setItem(TOKEN_KEY, this.token);
    else localStorage.removeItem(TOKEN_KEY);
  },
  saveView() {
    localStorage.setItem(VIEW_KEY, this.view);
  },
  saveSort() {
    localStorage.setItem(SORT_KEY, this.sort);
  },
  /** 会话过期:清空并回到登录页。 */
  expire() {
    this.token = "";
    this.user = null;
    this.shares = [];
    this.share = null;
    this.save();
    import("../views/shell.js").then((m) => m.showLogin());
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

/** 离线任务状态文案(ed2k 解析态有专属文案)。 */
export const OFFLINE_STATE_TEXT = {
  parsing: "解析中(获取文件列表)...",
  awaiting_select: "等待选择文件",
  downloading: "下载中",
  completed: "已完成",
  failed: "失败",
  cancelled: "已取消",
};
export const OFFLINE_STATE_TEXT_ED2K = { parsing: "查找下载来源..." };
