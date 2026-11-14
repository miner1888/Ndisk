// 管理后台:用户 / 共享目录 / HTTPS 证书 / 修改密码
import { api } from "./api.js";
import { session, esc } from "./store.js";
import { ICN } from "./icons.js";
import { toast, openModal } from "./ui.js";
import { loadFiles } from "./browser.js";
import { hostDirPicker } from "./dialogs.js";

async function reloadShares() {
  const r = await api("GET", "/api/shares");
  session.shares = r.shares || [];
  if (session.shares.length) {
    if (!session.shares.some((x) => session.share && x.id === session.share.id)) {
      session.share = session.shares[0];
      session.path = "";
      session.filter = "";
    }
  } else {
    session.share = null;
    session.path = "";
  }
}

export function renderAdmin() {
  document.getElementById("app").innerHTML = `
  <div class="admin-wrap">
    <div class="admin-top">
      <button class="btn mini ghost" id="admin-back">${ICN.back}返回文件</button>
      <span class="admin-title" id="admin-title">用户管理</span>
    </div>
    <div class="admin-body">
      <div class="admin-nav">
        <div class="nav-item active" data-pane="users">${ICN.users}用户管理</div>
        <div class="nav-item" data-pane="shares">${ICN.folder}共享目录</div>
        <div class="nav-item" data-pane="maint">${ICN.gear}系统维护</div>
        <div class="nav-item" data-pane="tls">${ICN.lock}HTTPS 证书</div>
        <div class="nav-item" data-pane="account">${ICN.key}我的密码</div>
      </div>
      <div class="admin-pane" id="pane"></div>
    </div>
  </div>`;
  const TITLES = { users: "用户管理", shares: "共享目录", maint: "系统维护", tls: "HTTPS 证书", account: "修改密码" };
  document.getElementById("admin-back").onclick = () => import("./main.js").then((m) => m.renderApp());
  document.querySelectorAll(".nav-item").forEach((el) => {
    el.onclick = () => {
      document.querySelectorAll(".nav-item").forEach((x) => x.classList.remove("active"));
      el.classList.add("active");
      document.getElementById("admin-title").textContent = TITLES[el.dataset.pane] || "";
      pane(el.dataset.pane);
    };
  });
  pane("users");
}

async function pane(p) {
  const el = document.getElementById("pane");
  if (p === "users") return usersPane(el);
  if (p === "shares") return sharesPane(el);
  if (p === "maint") return maintPane(el);
  if (p === "tls") return tlsPane(el);
  if (p === "account") return passwordPane(el);
}

/** 系统维护:硬件 / 转码方式、缓存占用与清理 */
async function maintPane(el) {
  el.innerHTML = '<div class="center"><div class="spinner"></div></div>';
  let st;
  try {
    st = await api("GET", "/api/admin/status");
  } catch (e) {
    return (el.innerHTML = `<p style="color:var(--danger)">${esc(e.message)}</p>`);
  }
  const cache = st.cache || {};
  const rows = [
    ["转码方式", "CPU · libx264(软件编码)"],
    ["ffmpeg", esc(st.ffmpeg || "未安装")],
    ["数据目录", esc(st.data_dir || "")],
    ["上传分片", fmtBytes(st.chunk_size)],
  ];
  const trOn = !!st.transcode_replace;
  const trDisabled = !st.ffmpeg;
  el.innerHTML = `
    <div class="card">
      <h3>运行环境</h3>
      <table class="admin-table">
        ${rows.map(([k, v]) => `<tr><th style="width:120px">${k}</th><td>${v}</td></tr>`).join("")}
        <tr><th style="width:120px">自动转码</th><td>
          <label class="check"><input type="checkbox" id="tr-toggle" ${trOn ? "checked" : ""} ${trDisabled ? "disabled" : ""}>
          上传视频自动转为 MP4 并替换原文件</label>
        </td></tr>
      </table>
      <div class="lbl" style="margin-top:8px">${trDisabled
        ? "未安装 ffmpeg,自动转码不可用。安装 ffmpeg 后刷新本页即可开启。"
        : "开启后,新上传的非 MP4/WebM 视频将在后台自动转为 H.264 MP4 并替换原文件(逐个排队转码,失败保留原文件);关闭则不转换,保留原样。默认关闭。"}</div>
      <div class="lbl" style="margin-top:8px">视频转码使用 ffmpeg 的 CPU 软件编码(libx264),不依赖显卡驱动与环境配置;未安装 ffmpeg 时,需要转码的视频无法在线播放(仍可下载)。</div>
    </div>
    <div class="card">
      <h3>媒体缓存</h3>
      <table class="admin-table">
        <tr><th style="width:120px">总计</th><td><b id="cache-total">${fmtBytes(cache.bytes)}</b> / ${cache.files || 0} 个文件</td></tr>
        <tr><th>缩略图</th><td>${fmtBytes(cache.thumb_bytes)} / ${cache.thumbs || 0} 个</td></tr>
        <tr><th>转码产物</th><td>${fmtBytes(cache.transcode_bytes)} / ${cache.transcodes || 0} 个</td></tr>
        <tr><th>缓存目录</th><td>${cache.dirs || 0} 个</td></tr>
      </table>
      <div style="display:flex;gap:10px;align-items:center;margin-top:10px;flex-wrap:wrap">
        <button class="btn" id="cache-clear">清理缓存</button>
        <span id="cache-msg" style="color:var(--muted);font-size:13px"></span>
      </div>
      <div class="lbl" style="margin-top:8px">缓存是视频转码产物与图片/视频缩略图,随时可以清理,播放或浏览时会按需重新生成;源文件被删除、重命名或覆盖时,对应缓存也会自动回收,超过 30 天未使用的缓存会自动过期。</div>
    </div>`;
  document.getElementById("cache-clear").onclick = async (ev) => {
    ev.target.disabled = true;
    document.getElementById("cache-msg").textContent = "清理中...";
    try {
      const r = await api("POST", "/api/admin/cache/clear", {});
      toast(`已清理缓存,释放 ${fmtBytes(r.freed)}`, true);
      maintPane(el);
    } catch (e) {
      document.getElementById("cache-msg").textContent = e.message;
      ev.target.disabled = false;
    }
  };
  const trToggle = document.getElementById("tr-toggle");
  if (trToggle) trToggle.onchange = async (ev) => {
    const on = ev.target.checked;
    try {
      await api("POST", "/api/admin/transcode-replace", { enabled: on });
      toast(on ? "已开启:上传视频将自动转码替换原文件" : "已关闭自动转码替换", true);
    } catch (e) {
      ev.target.checked = !on;
      toast(e.message);
    }
  };
}

function fmtBytes(n) {
  if (!n) return "0 B";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = Number(n);
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(i === 0 ? 0 : 1)} ${u[i]}`;
}

async function passwordPane(el) {
  el.innerHTML = `
    <div class="card">
      <h3>修改我的密码</h3>
      <div class="frm-row">
        <input id="cp-old" type="password" placeholder="原密码">
        <input id="cp-new" type="password" placeholder="新密码">
        <button class="btn" id="cp-btn">保存</button>
      </div>
    </div>`;
  document.getElementById("cp-btn").onclick = async () => {
    try {
      await api("POST", "/api/auth/password", {
        old_password: document.getElementById("cp-old").value,
        password: document.getElementById("cp-new").value,
      });
      toast("密码已更新", true);
    } catch (e) {
      toast(e.message);
    }
  };
}

async function usersPane(el) {
  el.innerHTML = '<div class="center"><div class="spinner"></div></div>';
  let users;
  try {
    users = await api("GET", "/api/users");
  } catch (e) {
    return (el.innerHTML = `<p style="color:var(--danger)">${esc(e.message)}</p>`);
  }
  el.innerHTML = `
    <div class="card">
      <h3>创建用户</h3>
      <div class="frm-row">
        <input id="nu-name" type="text" placeholder="用户名">
        <input id="nu-pass" type="password" placeholder="密码">
        <label class="check"><input type="checkbox" id="nu-admin"> 管理员</label>
        <button class="btn" id="nu-btn">创建</button>
      </div>
    </div>
    <div class="card">
      <h3>用户列表</h3>
      <div style="font-size:12px;color:var(--muted);margin-bottom:10px">勾选设置权限:上传 / 下载 / 删除 / 建文件夹</div>
      ${users.map((u) => `
      <div class="user-row">
        <div class="u-head">
          <b>${esc(u.username)}</b>
          <span class="rule-access ${u.is_admin ? "admin" : "login"}">${u.is_admin ? "管理员" : "普通"}</span>
          <span class="u-acts">
            <button class="btn mini ghost" data-reset="${u.id}">${ICN.key}重置密码</button>
            <button class="btn mini danger" data-del="${u.id}">${ICN.trash}删除</button>
          </span>
        </div>
        <div class="u-flags">${["can_upload", "can_download", "can_delete", "can_mkdir"].map((f) =>
          `<label class="check"><input type="checkbox" class="flag" data-u="${u.id}" data-f="${f}" ${u[f] ? "checked" : ""}>${
            f === "can_upload" ? "上传" : f === "can_download" ? "下载" : f === "can_delete" ? "删除" : "建目录"
          }</label>`).join("")}</div>
      </div>`).join("")}
    </div>`;
  el.querySelectorAll(".flag").forEach((c) => {
    c.onchange = async () => {
      const u = users.find((x) => x.id == c.dataset.u);
      const permit = { can_upload: u.can_upload, can_download: u.can_download, can_delete: u.can_delete, can_mkdir: u.can_mkdir };
      permit[c.dataset.f] = c.checked;
      try {
        await api("PATCH", `/api/users/${c.dataset.u}`, { permit });
        u[c.dataset.f] = c.checked;
        toast("已保存", true);
      } catch (e) {
        c.checked = !c.checked;
        toast(e.message);
      }
    };
  });
  el.querySelectorAll("[data-reset]").forEach((b) => {
    b.onclick = async () => {
      const pw = window.prompt("为这个用户输入新密码:");
      if (!pw) return;
      try {
        await api("PATCH", `/api/users/${b.dataset.reset}`, { password: pw });
        toast("已重置", true);
      } catch (e) {
        toast(e.message);
      }
    };
  });
  el.querySelectorAll("[data-del]").forEach((b) => {
    b.onclick = async () => {
      if (!window.confirm("确定删除该用户及其个人空间?")) return;
      try {
        await api("DELETE", `/api/users/${b.dataset.del}`);
        toast("已删除", true);
        usersPane(el);
      } catch (e) {
        toast(e.message);
      }
    };
  });
  document.getElementById("nu-btn").onclick = async () => {
    try {
      await api("POST", "/api/users", {
        username: document.getElementById("nu-name").value.trim(),
        password: document.getElementById("nu-pass").value,
        is_admin: document.getElementById("nu-admin").checked,
      });
      toast("用户已创建", true);
      usersPane(el);
    } catch (e) {
      toast(e.message);
    }
  };
}

async function sharesPane(el) {
  el.innerHTML = '<div class="card"><h3>共享目录管理</h3><div id="sh-list"><div class="spinner"></div></div></div>';
  let shares;
  try {
    shares = await api("GET", "/api/shares");
  } catch (e) {
    return (el.querySelector("#sh-list").innerHTML = `<p style="color:var(--danger)">${esc(e.message)}</p>`);
  }
  const list = shares.shares || [];
  el.innerHTML = `
    <div class="card">
      <h3>新增共享目录(指向服务器主机上的已有目录)</h3>
      <div class="frm-row">
        <input id="sh-name" type="text" placeholder="显示名称" style="min-width:180px">
        <div style="display:flex;gap:6px;flex:1;min-width:260px">
          <input id="sh-path" type="text" placeholder="绝对路径,如 /srv/media" style="flex:1;min-width:200px;max-width:400px">
          <button class="btn mini ghost" id="hb-browse">浏览…</button>
        </div>
        <button class="btn" id="sh-add">添加目录</button>
      </div>
      <div id="sh-err" style="color:var(--danger);min-height:18px"></div>
      <div class="lbl" style="margin-top:4px">根目录默认为"登录可访问",可在下方针对子目录设定游客/登录/管理员访问级别。</div>
    </div>
    <div id="sh-cards"></div>`;
  document.getElementById("sh-add").onclick = async () => {
    const name = document.getElementById("sh-name").value.trim();
    const path = document.getElementById("sh-path").value.trim();
    if (!name || !path) {
      document.getElementById("sh-err").textContent = "请填写名称与路径";
      return;
    }
    try {
      await api("POST", "/api/shares", { name, host_path: path });
      toast("已添加共享目录", true);
      await reloadShares();
      syncShareView();
      sharesPane(el);
    } catch (e) {
      document.getElementById("sh-err").textContent = e.message;
    }
  };
  document.getElementById("hb-browse").onclick = () =>
    hostDirPicker((p) => {
      document.getElementById("sh-path").value = p;
      document.getElementById("sh-err").textContent = "";
    }, document.getElementById("sh-path").value.trim());
  const custom = list.filter((s) => !s.home);
  const cards = document.getElementById("sh-cards");
  if (!custom.length) {
    cards.innerHTML = '<div class="card"><p style="color:var(--muted)">暂无自定义共享目录</p></div>';
    return;
  }
  cards.innerHTML = custom.map((s) => `
    <div class="card" style="margin-bottom:10px">
      <div style="display:flex;align-items:center;gap:10px;flex-wrap:wrap">
        <b>${esc(s.name)}</b>
        <span style="font-size:12px;color:var(--muted)">${esc(s.host_path || "")}</span>
        <span class="rule-access ${esc(s.access || "login")}">${accessName(s.access)}</span>
        <div style="margin-left:auto;display:flex;gap:6px">
          <button class="btn mini ghost" data-rules="${s.id}">访问规则</button>
          <button class="btn mini ghost" data-edit="${s.id}">改路径</button>
          <button class="btn mini danger" data-shdel="${s.id}">删除</button>
        </div>
      </div>
      <div id="rules-${s.id}" class="hidden" style="margin-top:10px"></div>
    </div>`).join("");
  cards.querySelectorAll("[data-rules]").forEach((b) => {
    b.onclick = () => {
      const box = document.getElementById("rules-" + b.dataset.rules);
      box.classList.toggle("hidden");
      if (!box.classList.contains("hidden")) loadRules(b.dataset.rules);
    };
  });
  cards.querySelectorAll("[data-edit]").forEach((b) => {
    b.onclick = async () => {
      const s = custom.find((x) => x.id == b.dataset.edit);
      const p = window.prompt("新的目录路径(绝对路径)", s.host_path || "");
      if (!p) return;
      try {
        await api("PATCH", `/api/shares/${s.id}`, { host_path: p });
        toast("已更新", true);
        await reloadShares();
        syncShareView();
        sharesPane(el);
      } catch (e) {
        toast(e.message);
      }
    };
  });
  cards.querySelectorAll("[data-shdel]").forEach((b) => {
    b.onclick = async () => {
      if (!window.confirm("删除该共享配置?(不会删除服务器上的文件)")) return;
      try {
        await api("DELETE", `/api/shares/${b.dataset.shdel}`);
        toast("已删除", true);
        await reloadShares();
        syncShareView();
        sharesPane(el);
      } catch (e) {
        toast(e.message);
      }
    };
  });
}

function accessName(a) {
  if (a === "guest") return "游客可访问";
  if (a === "admin") return "仅管理员";
  if (a === "private") return "私有空间";
  return a === "login" || !a ? "登录可访问" : a;
}

function syncShareView() {
  const el = document.getElementById("shareList");
  if (el) {
    import("./main.js").then((m) => m.renderShareList());
    loadFiles();
  }
}

async function loadRules(sid) {
  const box = document.getElementById("rules-" + sid);
  if (!box) return;
  box.innerHTML = '<div class="spinner"></div>';
  let rules = [];
  try {
    const r = await api("GET", `/api/shares/${sid}/rules`);
    rules = r.rules || [];
  } catch (e) {
    box.innerHTML = `<p style="color:var(--danger)">${esc(e.message)}</p>`;
    return;
  }
  box.innerHTML = `
    <div class="frm-row">
      <input id="nr-${sid}-path" type="text" placeholder="子目录路径,根目录留空" style="flex:1">
      <select id="nr-${sid}-access">
        <option value="guest">游客可访问</option>
        <option value="login">登录可访问</option>
        <option value="admin">仅管理员</option>
      </select>
      <button class="btn mini" data-addrule="${sid}">添加规则</button>
    </div>
    <div class="lbl" style="margin-bottom:6px">规则向下继承,路径越具体优先级越高。</div>
    <div>
      ${rules.length ? rules.map((x) => `
        <div class="rule-row">
          <span class="rule-access ${esc(x.access)}">${accessName(x.access)}</span>
          <span>/ ${esc(x.rel_path || "(根目录)")}</span>
          <span style="margin-left:auto"><button class="btn mini ghost" data-delrule="${x.id}">删除</button></span>
        </div>`).join("") : '<span class="lbl">暂无规则(默认仅管理员可访问)</span>'}
    </div>`;
  box.querySelector(`[data-addrule="${sid}"]`).onclick = async () => {
    try {
      await api("POST", `/api/shares/${sid}/rules`, {
        rel_path: document.getElementById(`nr-${sid}-path`).value || "",
        access: document.getElementById(`nr-${sid}-access`).value,
      });
      toast("规则已保存", true);
      loadRules(sid);
    } catch (e) {
      toast(e.message);
    }
  };
  box.querySelectorAll("[data-delrule]").forEach((b) => {
    b.onclick = async () => {
      try {
        await api("DELETE", `/api/shares/${sid}/rules/${b.dataset.delrule}`);
        loadRules(sid);
      } catch (e) {
        toast(e.message);
      }
    };
  });
}

async function tlsPane(el) {
  el.innerHTML = '<div class="card"><h3>HTTPS 证书</h3><div id="tls-status"><div class="spinner"></div></div></div>';
  let st;
  try {
    st = await api("GET", "/api/tls");
  } catch (e) {
    return (el.innerHTML = `<p style="color:var(--danger)">${esc(e.message)}</p>`);
  }
  const cert = st.cert;
  el.innerHTML = `
    <div class="card">
      <h3>当前状态</h3>
      ${st.enabled
        ? `<p style="color:var(--primary-2);font-weight:600;display:flex;align-items:center;gap:6px">${ICN.check}HTTPS 已启用</p>
           <p>访问地址:<code>https://${esc(cert ? cert.domain : "你的域名")}${st.port ? ":" + st.port : ""}</code></p>
           <p style="color:var(--muted);font-size:13px">监听:${esc(st.tls_listen)} · 域名字段:${esc(cert ? cert.domain : "-")}</p>`
        : `<p style="color:var(--warn);display:flex;align-items:center;gap:6px">${ICN.warn}未启用 HTTPS。添加有效证书后立即生效,可随时删除。不影响原有 HTTP 访问。</p>`}
      ${cert ? `<button class="btn mini danger" id="tls-del">${ICN.trash}删除证书并停用 HTTPS</button>` : ""}
    </div>
    <div class="card">
      <h3>添加 / 更新证书</h3>
      <div class="tabs">
        <button class="tab on" data-tab="paste">粘贴证书内容</button>
        <button class="tab" data-tab="file">服务器证书文件</button>
      </div>
      <div id="tls-paste">
        <input id="tls-domain" type="text" placeholder="证书对应域名,如 file.example.com" style="width:100%;max-width:420px;margin-bottom:8px">
        <div class="lbl" style="font-size:12px;color:var(--muted)">证书 (-----BEGIN CERTIFICATE-----)</div>
        <textarea id="tls-cert" rows="6" style="width:100%;font-family:monospace;font-size:12px" placeholder="-----BEGIN CERTIFICATE-----..."></textarea>
        <div class="lbl" style="font-size:12px;color:var(--muted);margin-top:8px">私钥 (-----BEGIN PRIVATE KEY----- / RSA PRIVATE KEY / EC PRIVATE KEY)</div>
        <textarea id="tls-key" rows="6" style="width:100%;font-family:monospace;font-size:12px" placeholder="-----BEGIN PRIVATE KEY-----..."></textarea>
      </div>
      <div id="tls-file" style="display:none">
        <input id="tls-domain-f" type="text" placeholder="证书对应域名,如 file.example.com" style="width:100%;max-width:420px;margin-bottom:8px">
        <div class="frm-row">
          <input id="tls-cert-path" type="text" placeholder="证书文件路径,如 /etc/letsencrypt/live/example.com/fullchain.pem" style="flex:1;font-family:monospace;font-size:12px">
          <button class="btn mini ghost" id="tls-browse-cert">浏览…</button>
        </div>
        <div class="frm-row">
          <input id="tls-key-path" type="text" placeholder="私钥文件路径,如 /etc/letsencrypt/live/example.com/privkey.pem" style="flex:1;font-family:monospace;font-size:12px">
          <button class="btn mini ghost" id="tls-browse-key">浏览…</button>
        </div>
      </div>
      <div style="margin-top:10px"><button class="btn" id="tls-save">保存并启用 HTTPS</button></div>
      <div id="tls-err" style="color:var(--danger);min-height:18px;margin-top:4px"></div>
    </div>`;
  const errEl = document.getElementById("tls-err");
  const tabs = el.querySelectorAll(".tab");
  tabs.forEach((t) => {
    t.onclick = () => {
      tabs.forEach((x) => x.classList.toggle("on", x === t));
      document.getElementById("tls-paste").style.display = t.dataset.tab === "paste" ? "" : "none";
      document.getElementById("tls-file").style.display = t.dataset.tab === "file" ? "" : "none";
    };
  });
  const browse = (inputId) =>
    hostDirPicker((p) => {
      document.getElementById(inputId).value = p;
    }, document.getElementById(inputId).value, { pickFiles: true });
  el.querySelector("#tls-browse-cert").onclick = () => browse("tls-cert-path");
  el.querySelector("#tls-browse-key").onclick = () => browse("tls-key-path");
  document.getElementById("tls-save").onclick = async () => {
    errEl.textContent = "";
    const fileMode = !document.getElementById("tls-file").style.display;
    try {
      const body = fileMode
        ? {
            domain: document.getElementById("tls-domain-f").value.trim(),
            cert_path: document.getElementById("tls-cert-path").value.trim(),
            key_path: document.getElementById("tls-key-path").value.trim(),
          }
        : {
            domain: document.getElementById("tls-domain").value.trim(),
            cert_pem: document.getElementById("tls-cert").value,
            key_pem: document.getElementById("tls-key").value,
          };
      await api("PUT", "/api/tls", body);
      toast("证书已保存,HTTPS 已生效", true);
      tlsPane(el);
    } catch (e) {
      errEl.textContent = e.message;
    }
  };
  const del = document.getElementById("tls-del");
  if (del) {
    del.onclick = async () => {
      if (!window.confirm("确定删除证书并停用 HTTPS?")) return;
      try {
        await api("DELETE", "/api/tls");
        toast("已停用 HTTPS", true);
        tlsPane(el);
      } catch (e) {
        toast(e.message);
      }
    };
  }
}
