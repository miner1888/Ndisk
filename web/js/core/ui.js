// UI 基础件:toast、弹窗骨架、右键菜单、主题切换、复制、缩略图失败重试。
import { esc } from "./state.js";

const THEME_KEY = "nd_theme";

export function toast(msg, ok) {
  let box = document.getElementById("toasts");
  if (!box) {
    box = document.createElement("div");
    box.id = "toasts";
    document.body.appendChild(box);
  }
  const t = document.createElement("div");
  t.className = "toast " + (ok ? "ok" : "err");
  t.textContent = msg;
  box.appendChild(t);
  setTimeout(() => t.remove(), 3200);
}

export function themeInit() {
  if (window.matchMedia) {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const fn = () => {
      let cur = "auto";
      try { cur = localStorage.getItem(THEME_KEY) || "auto"; } catch (e) {}
      if (cur === "auto") themeApply("auto");
    };
    if (mq.addEventListener) mq.addEventListener("change", fn);
  }
}

export function themeApply(pref) {
  const dark = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
  document.documentElement.setAttribute("data-theme", pref === "auto" ? (dark ? "dark" : "light") : pref);
  try { localStorage.setItem(THEME_KEY, pref); } catch (e) {}
  document.querySelectorAll(".theme-switch .ts-btn").forEach((b) => b.classList.toggle("active", b.dataset.ts === pref));
}

export function themeSwitcher() {
  let cur = "auto";
  try { cur = localStorage.getItem(THEME_KEY) || "auto"; } catch (e) {}
  const btn = (ts, label, svg) => `<button class="ts-btn ${cur === ts ? "active" : ""}" data-ts="${ts}" title="${label}" type="button">${svg}</button>`;
  return `<div class="theme-switch" title="切换主题">
    ${btn("light", "浅色", THEME_ICONS.sun)}${btn("dark", "深色", THEME_ICONS.moon)}${btn("auto", "跟随系统", THEME_ICONS.auto)}
  </div>`;
}

export function themeBind(root) {
  (root || document).querySelectorAll(".theme-switch .ts-btn").forEach((b) => {
    b.onclick = () => themeApply(b.dataset.ts);
  });
}

const THEME_ICONS = {
  sun: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><circle cx="12" cy="12" r="4.2"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1l2.1-2.1M17 7l2.1-2.1"/></svg>`,
  moon: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M20 14.5A8 8 0 0 1 9.5 4 6 6 0 1 0 20 14.5z"/></svg>`,
  auto: `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9"><circle cx="12" cy="12" r="9"/><path d="M12 3a9 9 0 0 0 0 18z" fill="currentColor" stroke="none"/></svg>`,
};

/** 弹窗骨架:点击遮罩或 [data-close] 关闭。 */
export function openModal(html, cls) {
  const mask = document.createElement("div");
  mask.className = "modal-mask";
  mask.innerHTML = `<div class="modal${cls ? " " + cls : ""}"><div class="mhead"><span class="mt">预览</span><span class="close" data-close="1">${closeIco()}</span></div>${html}</div>`;
  document.body.appendChild(mask);
  mask.addEventListener("click", (ev) => {
    if (ev.target === mask || ev.target.closest("[data-close]")) mask.remove();
  });
  return mask;
}

function closeIco() {
  return `<svg class="ui-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>`;
}

let menuEl = null;
let menuOpenedAt = 0;

export function closeMenu() {
  if (menuEl) {
    menuEl.remove();
    menuEl = null;
  }
}

/** 在指针位置弹出操作菜单;items: [{icon,label,act} | {sep:true}] */
export function openMenuAt(ev, items) {
  closeMenu();
  const menu = document.createElement("div");
  menu.className = "ctx-menu";
  menu.innerHTML = items
    .map((it) => (it.sep ? '<div class="ctx-sep"></div>' : `<button class="ctx-item"><span class="ctx-ico">${it.icon || ""}</span><span>${esc(it.label)}</span></button>`))
    .join("");
  const btns = menu.querySelectorAll(".ctx-item");
  btns.forEach((b, i) => {
    b.onclick = () => {
      closeMenu();
      const acts = items.filter((x) => !x.sep);
      if (acts[i] && acts[i].act) acts[i].act();
    };
  });
  document.body.appendChild(menu);
  const r = menu.getBoundingClientRect();
  menu.style.left = Math.max(6, Math.min(ev.clientX, window.innerWidth - r.width - 10)) + "px";
  menu.style.top = Math.max(6, Math.min(ev.clientY, window.innerHeight - r.height - 10)) + "px";
  menuEl = menu;
  menuOpenedAt = Date.now();
}

document.addEventListener("click", () => {
  if (Date.now() - menuOpenedAt < 60) return;
  closeMenu();
});
document.addEventListener("scroll", () => {
  if (Date.now() - menuOpenedAt > 120) closeMenu();
}, true);

export async function copyText(text, okMsg) {
  try {
    await navigator.clipboard.writeText(text);
    toast(okMsg || "已复制", true);
    return;
  } catch (e) {}
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    toast(okMsg || "已复制", true);
    ta.remove();
  } catch (e2) {
    toast("复制失败,请手动复制: " + text);
  }
}

/** 缩略图加载失败:重试 3 次(6s/3s/1.2s 间隔),仍失败则回退到文件图标。 */
window.thumbErr = (img) => {
  const n = parseInt(img.dataset.tr || "0", 10);
  if (n <= 0) {
    img.closest(".thumb").classList.add("fail");
    return;
  }
  img.dataset.tr = String(n - 1);
  const wait = n > 2 ? 1200 : n > 1 ? 3000 : 6000;
  setTimeout(() => {
    if (img.isConnected) img.src = img.dataset.src + (img.dataset.src.includes("?") ? "&" : "?") + "r=" + Date.now();
  }, wait);
};
