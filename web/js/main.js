// 引导:登录页 / 主界面外壳 / 启动流程
import { api } from "./api.js";
import { session, esc } from "./store.js";
import { ICN } from "./icons.js";
import { toast, themeSwitcher, themeBind, themeInit, openMenuAt } from "./ui.js";
import { loadFiles, renderToolbar, closeSidebar } from "./browser.js";
import { bindDropUpload } from "./uploader.js";
import { renderAdmin } from "./admin.js";

themeInit();

export function renderShareList() {
  const el = document.getElementById("shareList");
  if (!el) return;
  if (!session.shares.length) {
    el.innerHTML = '<div style="padding:14px;color:var(--muted);font-size:13px">无可用空间</div>';
    return;
  }
  const accessName = (a) => (a === "guest" ? "游客可访问" : a === "admin" ? "仅管理员" : a === "private" ? "私有" : "登录可访问");
  // 空间分三类:我的个人空间 / 其他用户空间(仅管理员可见)/ 管理员发布的共享目录
  const mineHome = session.shares.filter((s) => s.kind === "home" && s.mine);
  const otherHome = session.shares.filter((s) => s.kind === "home" && !s.mine);
  const shared = session.shares.filter((s) => s.kind !== "home");
  const item = (s, badge) => `
      <div class="share-item ${!session.albumView && session.share && session.share.id === s.id ? "active" : ""}" data-share="${s.id}">
        <span class="ico">${s.kind === "home" ? ICN.home : ICN.folder}</span>
        <span>${esc(s.name)}</span>
        <span class="share-badge">${badge}</span>
      </div>`;
  // 相册入口固定挂在个人空间下方,只聚合个人空间里的图片
  const albumItem = mineHome.length
    ? `<div class="share-item album-item ${session.albumView ? "active" : ""}" data-album="1">
        <span class="ico">${ICN.album}</span>
        <span>相册</span>
      </div>`
    : "";
  const section = (title, list, badge, extra) =>
    list.length ? `<div class="side-sub">${title}</div>${list.map((s) => item(s, badge(s))).join("")}${extra || ""}` : "";
  el.innerHTML = [
    section("我的空间", mineHome, () => "我的", albumItem),
    section("用户空间", otherHome, (s) => esc(s.owner_name || "私有")),
    section("共享空间", shared, (s) => accessName(s.access)),
  ].join("");
  el.querySelectorAll(".share-item[data-album]").forEach((node) => {
    node.onclick = () => {
      session.share = mineHome[0];
      session.albumReturnPath = session.path;
      // 先置状态再重绘侧栏,否则高亮会停留在个人空间
      session.albumView = true;
      renderShareList();
      import("./album.js").then((m) => m.loadAlbum());
      closeSidebar();
    };
  });
  el.querySelectorAll(".share-item[data-share]").forEach((node) => {
    node.onclick = () => {
      const s = session.shares.find((x) => x.id == node.dataset.share);
      if (s) {
        session.share = s;
        session.albumView = false;
        session.path = "";
        session.filter = "";
        renderShareList();
        loadFiles();
        closeSidebar();
      }
    };
  });
}

export function renderApp() {
  document.getElementById("app").innerHTML = `
  <div class="shell">
    <div class="sidebar" id="sidebar">
      <div class="brand">
        <span class="brand-mark"><img src="/static/logo.svg" alt="NDisk"></span>
        <span class="brand-name">云盘</span>
        ${session.user && session.user.is_admin ? '<span class="me">管理员</span>' : ""}
        <button class="drawer-close" id="drawer-close" title="收起">${ICN.close}</button>
      </div>
      <div class="share-list" id="shareList"></div>
      <div class="foot">
        <button class="who" id="whoBtn" type="button" title="账号操作">
          <span class="who-avatar">${session.user ? esc(session.user.username.slice(0, 1).toUpperCase()) : "客"}</span>
          <span class="who-name">${session.user ? esc(session.user.username) : "游客模式"}</span>
        </button>
        <div class="foot-theme" id="foot-theme"></div>
      </div>
    </div>
    <div class="side-mask" id="sideMask"></div>
    <div class="main">
      <div class="toolbar" id="toolbar"></div>
      <div id="selbar" class="selbar hidden"></div>
      <div class="files" id="files"></div>
    </div>
  </div>
  <div class="upload-panel hidden" id="uploadPanel"></div>
  <div class="fab-wrap" id="fabWrap">
    <div class="fab-menu" id="fabMenu"></div>
    <button class="fab" id="fabBtn" title="操作">${ICN.plus}</button>
  </div>`;
  // 点击头像弹出账号菜单(管理后台 / 退出登录;游客为登录)
  const who = document.getElementById("whoBtn");
  if (who) who.onclick = (ev) => {
    const r = who.getBoundingClientRect();
    const items = [];
    if (session.user && session.user.is_admin) items.push({ icon: ICN.gear, label: "管理后台", act: () => renderAdmin() });
    items.push({ icon: ICN.info, label: "使用帮助", act: () => import("./dialogs.js").then((m) => m.helpModal()) });
    if (session.user) items.push({ icon: ICN.logout, label: "退出登录", act: () => logout() });
    else items.push({ icon: ICN.login, label: "登录", act: () => showLogin() });
    openMenuAt({ clientX: r.left, clientY: r.top - 6 }, items);
  };
  const ft = document.getElementById("foot-theme");
  if (ft) {
    ft.innerHTML = themeSwitcher();
    themeBind(ft);
  }
  const sk = document.getElementById("sideMask");
  if (sk) sk.onclick = closeSidebar;
  const fb = document.getElementById("fabBtn");
  if (fb) fb.onclick = () => document.getElementById("fabWrap").classList.toggle("open");
  const dc = document.getElementById("drawer-close");
  if (dc) dc.onclick = closeSidebar;
  renderShareList();
  renderToolbar();
  loadFiles();
  bindDropUpload();
}

export function showLogin(guestHint) {
  document.getElementById("app").innerHTML = `
    <div class="login-page">
      <div class="bg-orb o1"></div><div class="bg-orb o2"></div><div class="bg-orb o3"></div>
      <aside class="login-hero">
        <div class="hero-top">
          <span class="hero-mark">云</span>
          <span class="hero-brand">云盘 · NDisk</span>
        </div>
        <div class="hero-mid">
          <h2 class="hero-title">你的随身<br><span class="accent">数字空间</span></h2>
          <p class="hero-sub">多用户网盘,支持分片断点上传、在线预览与编辑、目录级权限控制与安全分享链接。无论是个人备份还是团队协作,都能轻松应对。</p>
          <div class="features">
            <div class="feature"><span class="fic">${ICN.cloudUp}</span><span><b>断点续传</b><span class="fdesc">大文件分片上传,中断可续传</span></span></div>
            <div class="feature"><span class="fic">${ICN.play}</span><span><b>在线播放</b><span class="fdesc">视频音频直接预览,无需下载</span></span></div>
            <div class="feature"><span class="fic">${ICN.link}</span><span><b>安全分享</b><span class="fdesc">随机令牌链接,带访问统计</span></span></div>
            <div class="feature"><span class="fic">${ICN.shield}</span><span><b>权限控制</b><span class="fdesc">按目录设定游客/登录/管理员</span></span></div>
          </div>
        </div>
        <div class="hero-foot">© ${new Date().getFullYear()} 云盘 · NDisk · 基于 Rust / axum 构建</div>
      </aside>
      <main class="login-side">
        <div class="side-top">${themeSwitcher()}</div>
        <div class="side-mid">
          <div class="login-card">
            <div class="login-logo"><img src="/static/logo.svg" alt="NDisk"></div>
            <h1>欢迎回来</h1>
            <div class="sub">登录以继续访问你的文件</div>
            <label>用户名</label>
            <div class="fld"><span class="f-ico">${ICN.user}</span><input id="in-user" placeholder="请输入用户名" autocomplete="username"></div>
            <label>密码</label>
            <div class="fld"><span class="f-ico">${ICN.lock}</span><input id="in-pass" type="password" placeholder="请输入密码" autocomplete="current-password"></div>
            <label>验证码</label>
            <div class="fld"><span class="f-ico">${ICN.key}</span><input id="in-captcha" placeholder="输入右侧字符" maxlength="4" autocomplete="off"></div>
            <div class="captcha-row">
              <img id="cap-img" alt="验证码" title="看不清?点击刷新">
              <input type="hidden" id="cap-id">
              <button type="button" class="btn mini" id="cap-refresh">换一张</button>
            </div>
            <div class="login-actions">
              <button class="btn full" id="btn-login">${ICN.login}登录</button>
              <div class="divider">或</div>
              <button class="guest-link" id="btn-guest" type="button">以游客身份浏览公开目录</button>
            </div>
            <div class="err" id="login-err">${guestHint ? esc(guestHint) : ""}</div>
          </div>
        </div>
      </main>
    </div>`;
  themeBind(document);
  loadCaptcha();
  document.getElementById("btn-login").onclick = doLogin;
  document.getElementById("btn-guest").onclick = async () => {
    try {
      const r = await api("GET", "/api/shares");
      if (typeof r.direct_max === "number") session.directMax = r.direct_max;
      if (r.shares && r.shares.length) {
        session.shares = r.shares;
        session.share = r.shares[0];
        session.path = "";
        renderApp();
      } else {
        toast("暂无可供游客访问的目录");
      }
    } catch (e) {
      toast(e.message);
    }
  };
  const toPass = () => document.getElementById("in-pass").focus();
  document.getElementById("in-user").addEventListener("keydown", (e) => {
    if (e.key === "Enter") toPass();
  });
  document.getElementById("in-pass").addEventListener("keydown", (e) => {
    if (e.key === "Enter") doLogin();
  });
  document.getElementById("in-captcha").addEventListener("keydown", (e) => {
    if (e.key === "Enter") doLogin();
  });
  document.getElementById("cap-img").onclick = loadCaptcha;
  document.getElementById("cap-refresh").onclick = loadCaptcha;
}

async function loadCaptcha() {
  const img = document.getElementById("cap-img");
  try {
    const r = await api("GET", "/api/captcha");
    document.getElementById("cap-id").value = r.id;
    img.src = "data:image/svg+xml;base64," + btoa(unescape(encodeURIComponent(r.svg)));
    img.style.opacity = 1;
  } catch (e) {
    img.style.opacity = 0.4;
  }
}

async function doLogin() {
  const u = document.getElementById("in-user").value.trim();
  const p = document.getElementById("in-pass").value;
  const capId = document.getElementById("cap-id").value;
  const cap = document.getElementById("in-captcha").value.trim();
  try {
    const r = await api("POST", "/api/auth/login", { username: u, password: p, captcha_id: capId, captcha: cap });
    session.token = r.token;
    session.user = r.user;
    session.save();
    await bootApp();
  } catch (e) {
    document.getElementById("login-err").textContent = e.message;
    document.getElementById("in-captcha").value = "";
    loadCaptcha();
  }
}

function logout() {
  api("POST", "/api/auth/logout").catch(() => {});
  session.reset();
  showLogin();
}

async function bootApp() {
  if (session.token) {
    try {
      session.user = await api("GET", "/api/auth/me");
    } catch (e) {
      session.reset();
    }
  }
  if (!session.token) {
    try {
      const r = await api("GET", "/api/shares");
      if (r.shares && r.shares.length) {
        session.shares = r.shares;
        session.share = r.shares[0];
        session.path = "";
        renderApp();
        return;
      }
    } catch (e) {}
    return showLogin();
  }
  const r = await api("GET", "/api/shares");
  session.shares = r.shares || [];
  if (typeof r.direct_max === "number") session.directMax = r.direct_max;
  if (session.shares.length) session.share = session.shares[0];
  session.path = "";
  renderApp();
}

export function boot() {
  if (session.token) {
    api("GET", "/api/auth/me")
      .then((u) => {
        session.user = u;
        bootApp();
      })
      .catch(() => {
        session.reset();
        bootApp();
      });
  } else {
    bootApp();
  }
}

boot();
