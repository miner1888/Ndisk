// 图标体系:通用 UI 线性图标 + 按扩展名分类的文件页图标(60+ 格式)
// 渐变 ID 按颜色哈希生成,避免 SVG 内重复

const gId = (p) => "g" + p.replace(/[^a-z0-9]/gi, "");
const linear = (id, c1, c2) => `<linearGradient id="${id}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${c1}"/><stop offset="1" stop-color="${c2}"/></linearGradient>`;

export const ICN = {
  folder: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h3.5l2 2H19a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>`,
  folderOpen: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M3 8a2 2 0 0 1 2-2h3.5l2 2H19a2 2 0 0 1 2 2v1H3z"/><path d="M3 9h18l-2 9a2 2 0 0 1-2 1.5H5a2 2 0 0 1-2-1.5z"/></svg>`,
  home: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M4 10.5L12 4l8 6.5V20a1 1 0 0 1-1 1h-4v-6h-6v6H5a1 1 0 0 1-1-1z"/></svg>`,
  file: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M14 3v5h5"/><path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/></svg>`,
  download: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v11"/><path d="M7 12l5 5 5-5"/><path d="M5 19h14"/></svg>`,
  move: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4v11"/><path d="M8 11l4 4 4-4"/><path d="M4 14v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4"/></svg>`,
  menu: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="M4 6h16M4 12h16M4 18h16"/></svg>`,
  upload: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 16V5"/><path d="M7 10l5-5 5 5"/><path d="M5 19h14"/></svg>`,
  share: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="6" cy="12" r="2.5"/><circle cx="18" cy="6" r="2.5"/><circle cx="18" cy="18" r="2.5"/><path d="M8.2 10.8l7.6-3.6M8.2 13.2l7.6 3.6"/></svg>`,
  link: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M9 14a4 4 0 0 0 5.5 0l3-3a4 4 0 0 0-5.5-5.5l-1 1"/><path d="M15 10a4 4 0 0 0-5.5 0l-3 3a4 4 0 0 0 5.5 5.5l1-1"/></svg>`,
  copy: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h8"/></svg>`,
  edit: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M16 4l4 4-10 10H6v-4z"/><path d="M14 6l4 4"/></svg>`,
  save: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M5 4h11l3 3v13a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z"/><path d="M8 4v5h7"/><path d="M8 14h8v6H8z"/></svg>`,
  trash: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16"/><path d="M10 4h4"/><path d="M6 7l1 13h10l1-13"/><path d="M10 11v6M14 11v6"/></svg>`,
  refresh: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M4 12a8 8 0 0 1 14-5.3L21 9"/><path d="M20 12a8 8 0 0 1-14 5.3L3 15"/><path d="M21 4v5h-5M3 20v-5h5"/></svg>`,
  plus: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>`,
  lock: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="4.5" y="10" width="15" height="11" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/></svg>`,
  key: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="8" cy="8" r="4"/><path d="M11 11l9 9M16 16l2-2M14 14l2-2"/></svg>`,
  user: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="8" r="4"/><path d="M4 21c0-4 4-7 8-7s8 3 8 7"/></svg>`,
  users: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="9" cy="8" r="3.5"/><path d="M2.5 21c0-3.5 3-6 6.5-6s6.5 2.5 6.5 6"/><path d="M16 4.5a3.5 3.5 0 0 1 0 7"/><path d="M17.5 15c2.5.5 4 2.8 4 6"/></svg>`,
  gear: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19 12a7 7 0 0 0-.2-1.6l2-1.5-2-3.4-2.3 1a7 7 0 0 0-2.7-1.6l-.4-2.5h-4l-.4 2.5a7 7 0 0 0-2.7 1.6l-2.3-1-2 3.4 2 1.5a7 7 0 0 0 0 3.2l-2 1.5 2 3.4 2.3-1a7 7 0 0 0 2.7 1.6l.4 2.5h4l.4-2.5a7 7 0 0 0 2.7-1.6l2.3 1 2-3.4-2-1.5a7 7 0 0 0 .2-1.6z"/></svg>`,
  grid: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7.5" height="7.5" rx="1.5"/><rect x="13.5" y="3" width="7.5" height="7.5" rx="1.5"/><rect x="3" y="13.5" width="7.5" height="7.5" rx="1.5"/><rect x="13.5" y="13.5" width="7.5" height="7.5" rx="1.5"/></svg>`,
  list: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="M8 6h13M8 12h13M8 18h13"/><circle cx="3.5" cy="6" r="1.2" fill="currentColor" stroke="none"/><circle cx="3.5" cy="12" r="1.2" fill="currentColor" stroke="none"/><circle cx="3.5" cy="18" r="1.2" fill="currentColor" stroke="none"/></svg>`,
  close: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>`,
  back: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M19 12H5"/><path d="M11 18l-6-6 6-6"/></svg>`,
  up: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19V5"/><path d="M5 12l7-7 7 7"/></svg>`,
  chevL: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 6l-6 6 6 6"/></svg>`,
  chevR: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 6l6 6-6 6"/></svg>`,
  check: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12l5 5 9-10"/></svg>`,
  info: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 7.5v.5"/></svg>`,
  warn: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3l9 16H3z"/><path d="M12 9v5M12 17v.5"/></svg>`,
  logout: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M14 4h4a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-4"/><path d="M3 12h12"/><path d="M11 8l4 4-4 4"/></svg>`,
  login: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M10 4H6a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h4"/><path d="M11 12h12"/><path d="M17 8l4 4-4 4"/></svg>`,
  dots: `<svg class="ui-ico" viewBox="0 0 24 24" fill="currentColor" stroke="none"><circle cx="5" cy="12" r="1.9"/><circle cx="12" cy="12" r="1.9"/><circle cx="19" cy="12" r="1.9"/></svg>`,
  cert: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="3" width="16" height="14" rx="2"/><circle cx="9" cy="9" r="2.5"/><path d="M14 7h3M14 11h3"/><path d="M9 17l3 4 3-4"/></svg>`,
  search: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.4-4.4"/></svg>`,
  cloudUp: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M7 14a4 4 0 0 1 .6-7.9 5 5 0 0 1 9.4 1.4A3.5 3.5 0 0 1 16 17"/><path d="M12 19v-7M9 13l3-3 3 3"/></svg>`,
  album: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M13 5H6a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h13a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2h-5"/><path d="M16 3v5h8M12 13l2-3 3 4"/></svg>`,
  play: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M10 8v8l7-4z" fill="currentColor" stroke="none"/></svg>`,
  pause: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M10 8.5v7M14 8.5v7" stroke-width="2.4"/></svg>`,
  shield: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3l8 3v6c0 4.5-3 7.5-8 9-5-1.5-8-4.5-8-9V6z"/><path d="M9 12l2 2 4-4"/></svg>`,
};

// ---------------- 文件类型图标 ----------------

// 文件页模板:白页 + 折角 + 顶部彩色色带(可带标签文字)+ 主体图形
function page(top, bottom, tag, glyph) {
  bottom = bottom || top;
  const gid = gId(top + bottom);
  const long = tag && tag.length > 3;
  const fs = long ? 8.2 : 11;
  const label = tag ? `<text x="32" y="20.2" text-anchor="middle" font-family="Arial,Helvetica,sans-serif" font-weight="800" font-size="${fs}" fill="#fff" letter-spacing="0.6">${tag}</text>` : "";
  return `<svg class="fico" viewBox="0 0 64 64" xmlns="http://www.w3.org/2000/svg">
    <defs>${linear(gid, top, bottom)}</defs>
    <path d="M14 6h25l11 11v40a2 2 0 0 1-2 2H14a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2z" fill="#fff" stroke="#dbe1f0" stroke-width="1.3"/>
    <path d="M39 6v11h11z" fill="#f0f3fb" stroke="#dbe1f0" stroke-width="1.3" stroke-linejoin="round"/>
    <path d="M14 6h25l11 11v6H12V8a2 2 0 0 1 2-2z" fill="url(#${gid})"/>
    <path d="M39 6v6a2 2 0 0 0 2 2h9" fill="none" stroke="rgba(255,255,255,.55)" stroke-width="1.1"/>
    ${label}${glyph || ""}
  </svg>`;
}

const IMG = `<rect x="19" y="28" width="26" height="22" rx="3" fill="none" stroke="#a5b4fc" stroke-width="2"/><circle cx="26" cy="35" r="2.6" fill="#a5b4fc"/><path d="M21 47l6-6 4.5 4.5 4-3.5 8 8" fill="none" stroke="#a5b4fc" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>`;
const VID = `<rect x="19" y="29" width="26" height="20" rx="3" fill="none" stroke="#c4b5fd" stroke-width="2"/><path d="M29 35.5v7l6-3.5z" fill="#c4b5fd"/>`;
const MUS = `<path d="M30 32v12" stroke="#5eead4" stroke-width="2.4" stroke-linecap="round" fill="none"/><path d="M30 32c4.5-1.2 7 1 7 4.2" fill="none" stroke="#5eead4" stroke-width="2.4" stroke-linecap="round"/><ellipse cx="26.5" cy="44.5" rx="3.5" ry="2.5" fill="#5eead4"/>`;
const ZIPI = `<rect x="21" y="28" width="22" height="22" rx="2.5" fill="none" stroke="#fcd34d" stroke-width="2"/><path d="M32 28v22" stroke="#fcd34d" stroke-width="2"/><path d="M32 32.5h3.2M32 37h3.2M32 41.5h3.2" stroke="#fcd34d" stroke-width="1.8" stroke-linecap="round"/><rect x="30.5" y="29" width="3" height="3" fill="#fcd34d"/>`;
const LINES = `<path d="M20 33h24M20 38h24M20 43h15" stroke="#94a3b8" stroke-width="2.4" stroke-linecap="round"/>`;
const MD = `<path d="M22 47V32l6 6 6-6v15" fill="none" stroke="#a5b4fc" stroke-width="2.4" stroke-linejoin="round" stroke-linecap="round"/><path d="M38 36l3.5 3.5L38 43" fill="none" stroke="#a5b4fc" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/>`;
const JSONI = `<path d="M27 32c-2.4 0-3.6 1.4-3.6 3.6v2.8c0 1.6-.8 2.4-2.4 2.4 1.6 0 2.4.8 2.4 2.4v2.8c0 2.2 1.2 3.6 3.6 3.6" fill="none" stroke="#f59e0b" stroke-width="2.2" stroke-linecap="round"/><path d="M37 32c2.4 0 3.6 1.4 3.6 3.6v2.8c0 1.6.8 2.4 2.4 2.4-1.6 0-2.4.8-2.4 2.4v2.8c0 2.2-1.2 3.6-3.6 3.6" fill="none" stroke="#f59e0b" stroke-width="2.2" stroke-linecap="round"/>`;
const YML = `<circle cx="27" cy="38" r="4.5" fill="none" stroke="#67e8f9" stroke-width="2.2"/><path d="M30.3 41.3l8.2 8.2" stroke="#67e8f9" stroke-width="2.4" stroke-linecap="round"/>`;
const SHELL = `<rect x="19" y="29" width="26" height="18" rx="2.5" fill="none" stroke="#86efac" stroke-width="2"/><path d="M24.5 39l4-3.2-4-3.2" fill="none" stroke="#86efac" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/><path d="M33 40.5h6.5" stroke="#86efac" stroke-width="2.2" stroke-linecap="round"/>`;
const PYI = `<path d="M28 29h7a3 3 0 0 1 3 3v6h-10a3 3 0 0 0-3 3v3a3 3 0 0 0 3 3" fill="none" stroke="#93c5fd" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/><path d="M36 49h-7a3 3 0 0 1-3-3v-6h10a3 3 0 0 0 3-3v-3a3 3 0 0 0-3-3" fill="none" stroke="#fcd34d" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/><circle cx="31" cy="32" r="1" fill="#93c5fd"/><circle cx="33" cy="46" r="1" fill="#fcd34d"/>`;
const CODEG = `<path d="M25 32l-4.5 5 4.5 5" fill="none" stroke="#fdba74" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/><path d="M39 32l4.5 5-4.5 5" fill="none" stroke="#fdba74" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/><path d="M35 30l-6 14" stroke="#fdba74" stroke-width="2.4" stroke-linecap="round"/>`;
const SQLG = `<ellipse cx="32" cy="33" rx="8.5" ry="2.8" fill="none" stroke="#7dd3fc" stroke-width="2"/><path d="M23.5 33v10.5c0 1.6 3.8 2.8 8.5 2.8s8.5-1.2 8.5-2.8V33" fill="none" stroke="#7dd3fc" stroke-width="2"/><path d="M23.5 38.2c0 1.6 3.8 2.8 8.5 2.8s8.5-1.2 8.5-2.8" fill="none" stroke="#7dd3fc" stroke-width="1.6" opacity=".55"/>`;
const EXEG = `<rect x="19" y="29" width="26" height="19" rx="2.5" fill="none" stroke="#1d4ed8" stroke-width="2"/><path d="M19 34.5h26" stroke="#1d4ed8" stroke-width="2"/><circle cx="22.6" cy="31.8" r="1" fill="#1d4ed8"/><circle cx="25.8" cy="31.8" r="1" fill="#1d4ed8"/><path d="M23.5 39.5l3.2 3.2-3.2 3.2" fill="none" stroke="#1d4ed8" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/><path d="M30.5 46h6.5" stroke="#1d4ed8" stroke-width="1.9" stroke-linecap="round"/>`;
const PKGG = `<path d="M18.5 27h27v4.5h-27z" fill="none" stroke="#0284c7" stroke-width="2" stroke-linejoin="round"/><rect x="20" y="31.5" width="24" height="16.5" rx="2" fill="none" stroke="#0284c7" stroke-width="2"/><rect x="29.5" y="35" width="5" height="4" rx="1" fill="#0284c7"/>`;
const GEARG = `<circle cx="32" cy="38" r="4.2" fill="none" stroke="#475569" stroke-width="2"/><path d="M32 30.5v3.2M32 42.3v3.2M24.5 38h3.2M36.3 38h3.2M26.6 32.6l2.3 2.3M35.1 41.1l2.3 2.3M37.4 32.6l-2.3 2.3M28.9 41.1l-2.3 2.3" stroke="#475569" stroke-width="2" stroke-linecap="round"/>`;
const ANDROIDG = `<path d="M25.5 40.5a6.5 6.5 0 0 1 13 0z" fill="#16a34a"/><path d="M26.8 34.6l-1.6-2.4M37.2 34.6l1.6-2.4" stroke="#16a34a" stroke-width="1.7" stroke-linecap="round"/><circle cx="28.9" cy="37.8" r="1" fill="#fff"/><circle cx="35.1" cy="37.8" r="1" fill="#fff"/><path d="M25.5 43.5v1.5M32 43.5v2M38.5 43.5v1.5" stroke="#16a34a" stroke-width="1.7" stroke-linecap="round"/>`;
const FONTG = `<path d="M25.5 45.5l5-15.5 5 15.5" fill="none" stroke="#7c3aed" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/><path d="M28.2 40.5h4.6" stroke="#7c3aed" stroke-width="2" stroke-linecap="round"/>`;
const BOOKG = `<path d="M20.5 31.5c3.8-1.6 7.7-1.6 11.5 0v15.5c-3.8-1.6-7.7-1.6-11.5 0z" fill="none" stroke="#db2777" stroke-width="2" stroke-linejoin="round"/><path d="M32 31.5c3.8-1.6 7.7-1.6 11.5 0v15.5c-3.8-1.6-7.7-1.6-11.5 0z" fill="none" stroke="#db2777" stroke-width="2" stroke-linejoin="round"/><path d="M32 31.5v15.5" stroke="#db2777" stroke-width="2"/>`;

// 蓝紫渐变文件夹(带标签页后板 + 浅色前板 + 高光)
export const FICO_FOLDER = (() => {
  const id = gId("folderBody"), idTab = gId("folderTab");
  return `<svg class="fico" viewBox="0 0 64 64" xmlns="http://www.w3.org/2000/svg">
    <defs>${linear(id, "#a3b0fa", "#7b89f4")}${linear(idTab, "#8290f3", "#6b79ec")}</defs>
    <path d="M7 12a3 3 0 0 1 3-3h11.5a3 3 0 0 1 2.3 1.1l3.2 4.4H54a3 3 0 0 1 3 3v5.5H7z" fill="url(#${idTab})"/>
    <path d="M7 24a3 3 0 0 1 3-3h44a3 3 0 0 1 3 3v25a3 3 0 0 1-3 3H10a3 3 0 0 1-3-3z" fill="url(#${id})"/>
    <path d="M8.5 21h47v1.8h-47z" fill="rgba(255,255,255,.42)"/>
    <path d="M7 48h50v1a3 3 0 0 1-3 3H10a3 3 0 0 1-3-3z" fill="rgba(40,50,140,.18)"/>
  </svg>`;
})();

const GROUPS = [
  [["pdf"], page("#f87171", "#ef4444", "PDF")],
  [["doc", "docx", "docm", "odt", "rtf"], page("#60a5fa", "#2563eb", "DOC")],
  [["xls", "xlsx", "xlsm", "ods", "csv", "tsv"], page("#4ade80", "#16a34a", "XLS")],
  [["ppt", "pptx", "odp"], page("#fb923c", "#ea580c", "PPT")],
  [["jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "avif", "ico", "tif", "tiff", "heic", "heif", "jfif"], page("#a78bfa", "#8b5cf6", "IMG", IMG)],
  [["mp4", "m4v", "webm", "ogv", "mov", "avi", "mkv", "flv", "wmv", "mpg", "mpeg", "ts", "3gp", "3g2", "ogm", "m2ts", "mts", "vob", "asf", "rmvb", "f4v"], page("#a78bfa", "#7c3aed", "VID", VID)],
  [["mp3", "wav", "ogg", "oga", "flac", "m4a", "aac", "opus", "wma", "amr"], page("#2dd4bf", "#0d9488", "MP3", MUS)],
  [["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst", "cab", "jar", "iso"], page("#fbbf24", "#d97706", "ZIP", ZIPI)],
  [["exe", "com", "scr"], page("#60a5fa", "#1d4ed8", "EXE", EXEG)],
  [["msi", "msix", "appx", "dmg", "pkg", "deb", "rpm", "appimage"], page("#38bdf8", "#0284c7", "PKG", PKGG)],
  [["dll", "so", "dylib", "bin", "elf", "sys", "o", "a"], page("#94a3b8", "#475569", "BIN", GEARG)],
  [["apk", "xapk", "ipa"], page("#4ade80", "#16a34a", "APK", ANDROIDG)],
  [["ttf", "otf", "ttc", "woff", "woff2", "eot"], page("#c084fc", "#9333ea", "FONT", FONTG)],
  [["epub", "mobi", "azw3"], page("#f472b6", "#db2777", "EPUB", BOOKG)],
  [["txt", "log", "ini", "cfg", "conf", "toml", "properties", "env"], page("#94a3b8", "#64748b", "TXT", LINES)],
  [["md", "markdown"], page("#a78bfa", "#6d28d9", "MD", MD)],
  [["json"], page("#fbbf24", "#ca8a04", "JSON", JSONI)],
  [["yaml", "yml"], page("#22d3ee", "#0891b2", "YML", YML)],
  [["sh", "bash", "zsh", "fish", "ps1", "bat", "cmd"], page("#34d399", "#059669", "SH", SHELL)],
  [["py", "pyw", "pyi", "ipynb"], page("#60a5fa", "#2563eb", "PY", PYI)],
  [["js", "mjs", "cjs", "jsx"], page("#fbbf24", "#ca8a04", "JS")],
  [["ts", "tsx"], page("#60a5fa", "#2563eb", "TS")],
  [["html", "htm"], page("#fb923c", "#ea580c", "HTML", CODEG)],
  [["css", "scss", "sass", "less"], page("#38bdf8", "#0284c7", "CSS")],
  [["xml", "xsl", "xsd"], page("#94a3b8", "#64748b", "XML")],
  [["sql", "db", "sqlite", "sqlite3"], page("#38bdf8", "#0284c7", "SQL", SQLG)],
  [["c", "h", "cpp", "hpp", "cc", "cxx"], page("#60a5fa", "#1d4ed8", "C++", CODEG)],
  [["java", "kt", "kts", "scala", "groovy"], page("#fb923c", "#ea580c", "JAVA", CODEG)],
  [["go"], page("#22d3ee", "#0891b2", "GO", CODEG)],
  [["rs"], page("#fdba74", "#c2410c", "RS", CODEG)],
  [["rb"], page("#f87171", "#dc2626", "RB", CODEG)],
  [["php"], page("#a78bfa", "#7c3aed", "PHP", CODEG)],
  [["cs"], page("#38bdf8", "#0284c7", "C#", CODEG)],
  [["swift", "dart", "lua", "pl", "pm", "r", "vue", "svelte", "asm"], page("#94a3b8", "#64748b", "CODE", CODEG)],
];

export function fileIcon(x) {
  if (x.is_dir) return FICO_FOLDER;
  const e = (x.ext || "").toLowerCase();
  for (const [exts, svg] of GROUPS) {
    if (exts.includes(e)) return svg;
  }
  return page("#cbd5e1", "#9ca3af", "FILE");
}

const IMAGE_EXTS = ["jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "avif", "ico"];
const VIDEO_EXTS = ["mp4", "m4v", "webm", "ogv", "mov", "avi", "mkv", "flv", "wmv", "mpg", "mpeg", "ts", "3gp", "m2ts", "mts"];
const AUDIO_EXTS = ["mp3", "wav", "ogg", "oga", "flac", "m4a", "aac", "opus", "wma", "amr"];
const ARCHIVE_EXTS = ["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst", "iso"];

export function kindOf(x) {
  if (x.is_dir) return "dir";
  const e = (x.ext || "").toLowerCase();
  if (IMAGE_EXTS.includes(e)) return "image";
  if (VIDEO_EXTS.includes(e)) return "video";
  if (AUDIO_EXTS.includes(e)) return "audio";
  if (ARCHIVE_EXTS.includes(e)) return "archive";
  return "doc";
}
