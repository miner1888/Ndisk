// 各类操作弹窗:新建文件夹 / 重命名 / 删除 / 分享 / 权限 / 详情 / 服务器目录选择器
import { api } from "./api.js";
import { session, esc, fmtSize, fmtTimeFull, childPath } from "./store.js";
import { ICN, FICO_FOLDER, fileIcon } from "./icons.js";
import { toast, openModal, copyText } from "./ui.js";
import { loadFiles } from "./browser.js";

export function mkdirModal() {
  const isAdmin = session.user && session.user.is_admin;
  const mask = openModal(`
    <div class="mk">
      <div class="mk-ico">${FICO_FOLDER}</div>
      <input id="mkdir-name" placeholder="请输入文件夹名称" autocomplete="off" maxlength="255">
      ${isAdmin ? `<div class="mk-row"><label>谁能访问</label>
        <select id="mkdir-access">
          <option value="inherit">跟随上级(默认)</option>
          <option value="guest">访客可访问(无需登录)</option>
          <option value="login">登录用户可访问</option>
          <option value="admin">仅管理员可访问</option>
        </select></div>` : ""}
      <div class="pf-actions">
        <button class="btn ghost" data-close="1">取消</button>
        <button class="btn" id="mkdir-ok">确认</button>
      </div>
    </div>`, "modal-xs");
  mask.querySelector(".mhead .mt").textContent = "新建文件夹";
  const inp = mask.querySelector("#mkdir-name");
  const okBtn = mask.querySelector("#mkdir-ok");
  inp.focus();
  const go = async () => {
    const name = inp.value.trim();
    if (!name) return toast("请输入文件夹名称");
    const acc = isAdmin ? mask.querySelector("#mkdir-access").value : "inherit";
    try {
      await api("POST", "/api/mkdir", { share_id: session.share.id, path: session.path, name });
      let ruleErr = null;
      if (acc !== "inherit") {
        try {
          await api("POST", `/api/shares/${session.share.id}/rules`, { rel_path: childPath(name), access: acc });
        } catch (e) {
          ruleErr = e;
        }
      }
      toast(ruleErr ? "文件夹已创建,但权限设置失败:" + ruleErr.message : "已创建", !ruleErr);
    } catch (e) {
      toast(e.message);
      return;
    }
    mask.remove();
    loadFiles();
  };
  okBtn.onclick = go;
  inp.addEventListener("keydown", (e) => {
    if (e.key === "Enter") go();
  });
}

export function renameModal(name) {
  const mask = openModal(`
    <div class="pf">
      <div class="pf-row"><label>原名称</label><span style="font-size:13px">${esc(name)}</span></div>
      <div class="pf-row"><label>新名称</label><input id="rn-name" value="${esc(name)}"></div>
      <div class="pf-actions">
        <button class="btn mini ghost" data-close="1">取消</button>
        <button class="btn" id="rn-ok">重命名</button>
      </div>
    </div>`, "modal-sm");
  mask.querySelector(".mhead .mt").textContent = "重命名";
  const inp = mask.querySelector("#rn-name");
  const okBtn = mask.querySelector("#rn-ok");
  inp.focus();
  inp.select();
  const go = async () => {
    const val = inp.value.trim();
    if (!val) return toast("名称不能为空");
    if (val === name) return mask.remove();
    okBtn.disabled = true;
    try {
      await api("POST", "/api/rename", { share_id: session.share.id, path: session.path, old: name, new: val });
      mask.remove();
      toast("已重命名", true);
      loadFiles();
    } catch (e) {
      okBtn.disabled = false;
      toast(e.message || "重命名失败");
    }
  };
  okBtn.onclick = go;
  inp.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") go();
  });
}

/** 调用后端移动接口;成功后刷新列表并提示 */
export async function moveItems(names, dest) {
  if (!names || !names.length || !session.share) return;
  try {
    const r = await api("POST", "/api/move", {
      share_id: session.share.id,
      path: session.path,
      names,
      dest,
    });
    if (r && r.errors && r.errors.length) {
      toast(`已移动 ${r.moved} 项,失败 ${r.errors.length} 项:${r.errors[0]}`, !r.moved);
    } else if (r && r.moved) {
      toast(`已移动 ${r.moved} 项`, true);
    }
    loadFiles();
  } catch (e) {
    toast(e.message || "移动失败");
  }
}

/** 移动到…:共享内文件夹选择器(面包屑 + 子文件夹列表) */
export function moveModal(names) {
  if (!names || !names.length) return;
  const srcPath = session.path;
  let cur = ""; // 从共享根开始浏览
  const mask = openModal(`
    <div class="mv">
      <div class="mv-crumb" id="mv-crumb"></div>
      <div class="mv-list" id="mv-list"></div>
      <div class="mv-actions">
        <button class="btn mini ghost" id="mv-up">${ICN.up}上一级</button>
        <span style="flex:1"></span>
        <button class="btn mini ghost" data-close="1">取消</button>
        <button class="btn" id="mv-ok">${ICN.move}移动到此处</button>
      </div>
    </div>`, "modal-sm");
  mask.querySelector(".mhead .mt").textContent = `移动 ${names.length} 项到…`;
  const crumbEl = mask.querySelector("#mv-crumb");
  const listEl = mask.querySelector("#mv-list");
  const upBtn = mask.querySelector("#mv-up");
  const okBtn = mask.querySelector("#mv-ok");

  const crumbsOf = (p) => {
    const parts = p ? p.split("/") : [];
    let acc = "";
    let html = `<a href="#" data-p="">根目录</a>`;
    parts.forEach((x) => {
      acc = acc ? acc + "/" + x : x;
      html += ` <span class="sp">›</span> <a href="#" data-p="${esc(acc)}">${esc(x)}</a>`;
    });
    return html;
  };

  async function render() {
    okBtn.disabled = true;
    upBtn.disabled = !cur;
    crumbEl.innerHTML = crumbsOf(cur);
    crumbEl.querySelectorAll("a").forEach((a) => {
      a.onclick = (ev) => {
        ev.preventDefault();
        cur = a.dataset.p || "";
        render();
      };
    });
    listEl.innerHTML = '<div class="center"><div class="spinner"></div></div>';
    try {
      const r = await api("GET", `/api/browse/${session.share.id}?path=${encodeURIComponent(cur)}`);
      const dirs = (r.entries || []).filter((x) => x.is_dir);
      // 源目录本身不可作为目标;同级直接被移动的文件夹不可进入(避免无效浏览)
      const blocked = new Set(names);
      const shown = dirs.filter((x) => !(cur === srcPath && blocked.has(x.name)));
      if (!shown.length) {
        listEl.innerHTML = '<div class="mv-empty">没有子文件夹</div>';
      } else {
        listEl.innerHTML = shown.map((x) => `
          <div class="mv-dir" data-name="${esc(x.name)}">
            <span class="ico">${ICN.folder}</span><span class="nm">${esc(x.name)}</span>
            <span class="go">${ICN.chevR}</span>
          </div>`).join("");
        listEl.querySelectorAll(".mv-dir").forEach((n) => {
          n.onclick = () => {
            const name = n.dataset.name;
            cur = cur ? cur + "/" + name : name;
            render();
          };
        });
      }
      okBtn.disabled = cur === srcPath;
      okBtn.title = okBtn.disabled ? "已在当前文件夹" : "";
    } catch (e) {
      listEl.innerHTML = `<div class="mv-empty" style="color:var(--danger)">${esc(e.message)}</div>`;
    }
  }

  upBtn.onclick = () => {
    if (!cur) return;
    cur = cur.split("/").slice(0, -1).join("/");
    render();
  };
  okBtn.onclick = () => {
    if (okBtn.disabled) return;
    mask.remove();
    moveItems(names, cur);
  };
  render();
}

export function deleteModal(names) {
  const hasDir = names.some((n) => session.selDir.has(n));
  const mask = openModal(`
    <div class="pf">
      <p style="font-size:14px;margin-bottom:10px">确认删除以下 <b>${names.length}</b> 项?此操作<b>不可恢复</b>:</p>
      <ul style="font-size:13px;color:var(--muted);margin:0 0 14px 18px;line-height:1.8">${names.map((n) => `<li>${esc(n)}</li>`).join("")}</ul>
      ${hasDir ? '<p class="pf-hint" style="margin-bottom:14px">包含文件夹,其下所有内容将一并删除</p>' : ""}
      <div class="pf-actions">
        <button class="btn mini ghost" data-close="1">取消</button>
        <button class="btn danger" id="del-ok">确认删除</button>
      </div>
    </div>`, "modal-sm");
  mask.querySelector(".mhead .mt").textContent = "删除";
  const okBtn = mask.querySelector("#del-ok");
  okBtn.onclick = async () => {
    okBtn.disabled = true;
    let done = 0, firstErr = null, failedName = "";
    for (const n of names) {
      try {
        await api("POST", "/api/delete", { share_id: session.share.id, path: childPath(n), recursive: true });
        done++;
      } catch (e) {
        if (!firstErr) { firstErr = e; failedName = n; }
      }
    }
    if (done > 0) loadFiles();
    if (firstErr && done < names.length) {
      okBtn.disabled = false;
      toast(`已删除 ${done}/${names.length} 项;"${failedName}"删除失败:${firstErr.message || "未知错误"}`);
      if (done > 0) return;
    }
    mask.remove();
    if (done === names.length) toast("已删除 " + names.length + " 项", true);
  };
}

export function shareModal(shareId, path, name) {
  const mask = openModal(`
    <div class="pf">
      <div class="pf-row" style="flex-wrap:wrap"><label>文件</label><b style="font-size:13px">${esc(name)}</b></div>
      <div class="pf-row" style="flex-wrap:wrap">
        <label>分享链接</label>
        <input id="fs-url" type="text" readonly placeholder="点击下方按钮生成链接">
        <button class="btn mini" id="fs-copy" disabled>复制</button>
      </div>
      <div id="fs-status" class="lbl" style="min-height:18px"></div>
      <div class="pf-actions">
        <button class="btn mini ghost" data-close="1">关闭</button>
        <button class="btn" id="fs-create">生成分享链接</button>
        <button class="btn mini danger hidden" id="fs-revoke">取消分享</button>
      </div>
    </div>`, "modal-sm");
  mask.querySelector(".mhead .mt").textContent = "分享链接";
  const url = mask.querySelector("#fs-url");
  const status = mask.querySelector("#fs-status");
  const createBtn = mask.querySelector("#fs-create");
  const revokeBtn = mask.querySelector("#fs-revoke");
  const copyBtn = mask.querySelector("#fs-copy");
  const setLink = (tok) => {
    url.value = location.origin + "/s/" + tok;
    copyBtn.disabled = false;
    createBtn.classList.add("hidden");
    revokeBtn.classList.remove("hidden");
  };
  const clearLink = () => {
    url.value = "";
    copyBtn.disabled = true;
    createBtn.classList.remove("hidden");
    revokeBtn.classList.add("hidden");
  };
  copyBtn.onclick = () => copyText(url.value, "链接已复制");
  createBtn.onclick = async () => {
    status.textContent = "";
    try {
      const r = await api("POST", "/api/fileshares", { share_id: shareId, path, ttl_days: 0 });
      setLink(r.url.split("/").pop());
      status.textContent = "任何人打开此链接即可访问或下载该文件,无需登录";
    } catch (e) {
      status.textContent = e.message;
    }
  };
  revokeBtn.onclick = async () => {
    const tok = url.value.split("/").pop();
    if (!tok) return;
    try {
      await api("DELETE", "/api/fileshares/" + tok);
      status.textContent = "已取消分享,链接已失效";
      clearLink();
    } catch (e) {
      status.textContent = e.message;
    }
  };
  api("GET", `/api/fileshares?share_id=${shareId}`)
    .then((r) => {
      const hit = (r.links || []).find((x) => x.path === path);
      if (hit) {
        setLink(hit.token);
        status.textContent = "该文件已有分享链接(已被访问 " + hit.hits + " 次)";
      }
    })
    .catch(() => {});
}

export async function folderPermModal() {
  const s = session.share;
  if (!s || !session.user || !session.user.is_admin) return;
  const target = session.path;
  const disp = target || "(根目录)";
  const mask = openModal(`
    <div class="pf">
      <div class="pf-row" style="flex-wrap:wrap"><label>文件夹</label><b>${esc(disp)}</b></div>
      <div class="pf-row"><label>谁能访问</label>
        <select id="perm-access">
          <option value="inherit">跟随上级(无单独设置)</option>
          <option value="guest">访客可访问(无需登录)</option>
          <option value="login">登录用户可访问</option>
          <option value="admin">仅管理员可访问</option>
        </select>
      </div>
      <div class="pf-hint">此设置对该文件夹及其下所有子目录生效,子目录可单独覆盖。未登录的访客只能看到标记为"访客可访问"的内容;根目录设为"访客可访问"后,未登录用户即可在侧边栏看到该共享。</div>
      <div class="pf-actions">
        <button class="btn mini ghost" data-close="1">关闭</button>
        <button class="btn" id="perm-save">保存</button>
      </div>
    </div>`, "modal-sm");
  mask.querySelector(".mhead .mt").textContent = "文件夹权限";
  let mine = null;
  try {
    const r = await api("GET", `/api/shares/${s.id}/rules`);
    mine = (r.rules || []).find((x) => x.rel_path === target) || null;
  } catch (e) {}
  const sel = mask.querySelector("#perm-access");
  sel.value = mine ? mine.access : "inherit";
  mask.querySelector("#perm-save").onclick = async () => {
    const v = sel.value;
    try {
      if (v === "inherit") {
        if (mine) await api("DELETE", `/api/shares/${s.id}/rules/${mine.id}`);
      } else {
        await api("POST", `/api/shares/${s.id}/rules`, { rel_path: target, access: v });
      }
      toast("权限已更新", true);
    } catch (e) {
      toast(e.message);
      return;
    }
    mask.remove();
    loadFiles();
  };
}

/** 详情弹窗:大图标 + 详细信息(大小 / 位置面包屑 / 时间),宽度紧凑 */
export async function detailModal(name, isdir, fullPath) {
  const path = fullPath ? fullPath : childPath(name);
  const icon = isdir ? FICO_FOLDER : fileIcon({ is_dir: false, ext: (path.split(".").pop() || "").toLowerCase() });
  const mask = openModal(`
    <div class="dt">
      <div class="dt-ico">${icon}</div>
      <div class="dt-sec">详细信息</div>
      <div id="dt-body"><div class="center" style="padding:26px"><div class="spinner"></div></div></div>
    </div>`, "modal-xs");
  mask.querySelector(".mhead .mt").textContent = name;
  const body = mask.querySelector("#dt-body");
  let r;
  try {
    r = await api("GET", `/api/file/detail/${session.share.id}?path=${encodeURIComponent(path)}`);
  } catch (e) {
    body.innerHTML = `<p class="pv-msg" style="color:var(--danger)">${esc(e.message)}</p>`;
    return;
  }
  const sizeLine = r.is_dir
    ? `${fmtSize(r.size)}(包含 ${r.files} 个文件,${r.folders} 个文件夹)`
    : fmtSize(r.size);
  const segs = (path.includes("/") ? path.slice(0, path.lastIndexOf("/")).split("/") : []).filter(Boolean);
  let crumb = `<a data-dt-path="">全部文件</a>`;
  let acc = "";
  segs.forEach((s) => {
    acc = acc ? acc + "/" + s : s;
    crumb += ` <span class="dt-sep">›</span> <a data-dt-path="${esc(acc)}">${esc(s)}</a>`;
  });
  body.innerHTML = `
    <div class="dt-row"><span class="dt-l">名称</span><span class="dt-v">${esc(r.name)}</span></div>
    <div class="dt-row"><span class="dt-l">大小</span><span class="dt-v">${esc(sizeLine)}</span></div>
    <div class="dt-row"><span class="dt-l">文件位置</span><span class="dt-v dt-path">${crumb}</span></div>
    <div class="dt-row"><span class="dt-l">创建时间</span><span class="dt-v">${fmtTimeFull(r.created)}</span></div>
    <div class="dt-row"><span class="dt-l">最后修改时间</span><span class="dt-v">${fmtTimeFull(r.mtime)}</span></div>`;
  body.querySelectorAll("[data-dt-path]").forEach((a) => {
    a.onclick = () => {
      session.albumView = false;
      session.path = a.dataset.dtPath || "";
      session.filter = "";
      mask.remove();
      loadFiles();
    };
  });
}

/** 服务器目录 / 证书文件选择器(管理员) */
export async function hostDirPicker(onPick, initial, opts) {
  const pickFiles = !!(opts && opts.pickFiles);
  const mask = openModal(`
    <div class="pf">
      <div class="hb-cur"><span style="color:var(--muted)">当前目录</span><b id="hb-path">/</b></div>
      <div class="pf-row"><input id="hb-input" type="text" placeholder="输入关键字过滤;或输入完整路径后回车直接跳转" style="width:100%"></div>
      <div id="hb-list" class="hb-list"><div class="spinner"></div></div>
      <div class="pf-actions">
        <button class="btn mini ghost" data-close="1">取消</button>
        <button class="btn mini ghost" id="hb-up">${ICN.up}上一级</button>
        <button class="btn" id="hb-ok">${pickFiles ? "选择此文件" : "选择此目录"}</button>
      </div>
    </div>`, "modal-md");
  mask.querySelector(".mhead .mt").textContent = pickFiles ? "选择服务器证书文件" : "选择服务器目录";
  const cur = { path: initial || "/" };
  const pathEl = mask.querySelector("#hb-path");
  const listEl = mask.querySelector("#hb-list");
  const inputEl = mask.querySelector("#hb-input");
  const upBtn = mask.querySelector("#hb-up");
  const okBtn = mask.querySelector("#hb-ok");

  const filterInput = () => {
    const kw = inputEl.value.trim().toLowerCase();
    listEl.querySelectorAll(".hb-dir, .hb-file").forEach((el) => {
      el.style.display = !kw || el.dataset.dir.toLowerCase().includes(kw) ? "" : "none";
    });
  };
  inputEl.addEventListener("input", filterInput);
  inputEl.addEventListener("keydown", (ev) => {
    if (ev.key !== "Enter") return;
    const v = inputEl.value.trim();
    if (v.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(v) || /^[a-zA-Z]:$/.test(v)) {
      cur.path = v;
      load();
    }
  });

  const renderDirs = (dirs, files) => {
    const rows = [];
    dirs.forEach((d) => rows.push(`<div class="hb-dir" data-dir="${esc(d)}"><span style="color:var(--warn);display:inline-flex">${ICN.folder}</span><span>${esc(d)}</span></div>`));
    files.forEach((f) => rows.push(`<div class="hb-file" data-dir="${esc(f)}"><span style="color:var(--muted);display:inline-flex">${ICN.file || ""}</span><span>${esc(f)}</span></div>`));
    if (!rows.length) {
      listEl.innerHTML = `<div style="padding:20px 0;color:var(--muted);text-align:center">${pickFiles ? "无子目录与证书文件" : "无子目录"}</div>`;
      return;
    }
    listEl.innerHTML = rows.join("");
    listEl.querySelectorAll(".hb-dir").forEach((el) => {
      el.onclick = () => {
        cur.path = joinPath(cur.path, el.dataset.dir);
        load();
      };
    });
    listEl.querySelectorAll(".hb-file").forEach((el) => {
      el.onclick = () => {
        onPick(joinPath(cur.path, el.dataset.dir));
        mask.remove();
      };
    });
  };

  const load = async () => {
    inputEl.disabled = true;
    listEl.innerHTML = '<div class="spinner"></div>';
    try {
      const r = await api("GET", "/api/browse-host?path=" + encodeURIComponent(cur.path) + (pickFiles ? "&files=1" : ""));
      cur.path = r.path;
      pathEl.textContent = r.path;
      okBtn.style.display = pickFiles ? "none" : "";
      upBtn.disabled = !r.parent || r.path === "/";
      upBtn.onclick = () => {
        if (!r.parent) return;
        cur.path = r.parent;
        load();
      };
      renderDirs(r.dirs || [], pickFiles ? r.files || [] : []);
    } catch (e) {
      listEl.innerHTML = `<p style="color:var(--danger);padding:16px 0">${esc(e.message)}</p>`;
    } finally {
      inputEl.disabled = false;
      inputEl.value = "";
      filterInput();
    }
  };
  okBtn.onclick = () => {
    onPick(cur.path);
    mask.remove();
  };
  await load();
}

function joinPath(a, b) {
  if (!b) return a;
  if (a === "/" && /^[a-zA-Z]:?$/.test(b)) return b.replace(/:$/, "") + ":\\";
  if (/^[a-zA-Z]:[\\/]/.test(a)) return a.replace(/[\\/]+$/, "") + "\\" + b;
  if (a === "/") return "/" + b.replace(/^\/+/, "");
  return a.replace(/\/+$/, "") + "/" + b.replace(/^\/+/, "");
}

/** 使用帮助:常用操作与 WebDAV 连接说明 */
export function helpModal() {
  const host = location.host;
  const davUrl = `${location.protocol}//${host}/dav/`;
  const mask = openModal(`
    <div class="help">
      <div class="help-sec">
        <div class="help-t">常用快捷键</div>
        <div class="help-row"><span>Ctrl+A</span>全选 / 取消全选</div>
        <div class="help-row"><span>F2</span>重命名选中项</div>
        <div class="help-row"><span>Delete</span>删除选中项</div>
        <div class="help-row"><span>Esc</span>关闭预览 / 弹窗</div>
        <div class="help-row"><span>双击 / 轻点</span>打开文件或进入文件夹</div>
        <div class="help-row"><span>右键</span>文件操作菜单(移动 / 分享 / 详情…)</div>
      </div>
      <div class="help-sec">
        <div class="help-t">WebDAV 连接</div>
        <div class="help-row">地址:<b>${esc(davUrl)}</b><button class="btn mini ghost" id="help-copy">${ICN.copy}复制</button></div>
        <div class="help-row">认证:你的登录用户名和密码</div>
        <div class="help-p">连接后看到的就是你自己的个人空间,可直接建文件夹 / 上传 / 同步;管理员发布的共享目录以文件夹形式列在根下,按目录权限读写。</div>
        <div class="help-row">Windows 映射网络驱动器:</div>
        <div class="help-p">文件资源管理器 → 此电脑 → 映射网络驱动器 → 填入上面的地址 → 勾选"使用其他凭据连接" → 输入账号密码。公网使用请先在管理后台配置 HTTPS。</div>
        <div class="help-row">rclone 示例:</div>
        <div class="help-code">rclone config create ndisk webdav url ${esc(davUrl)} vendor other user 你的账号 pass 你的密码
rclone ls ndisk:
rclone copy 本地目录 ndisk:目标目录</div>
        <div class="help-p">说明:单次上传 ≤ 分片大小(默认 8M)自动直传,大文件自动分片断点续传;下载支持断点续传;开启"自动转码替换"后,上传的视频会自动转为 MP4。</div>
      </div>
      <div class="help-actions"><button class="btn mini" data-close="1">知道了</button></div>
    </div>`, "modal-md");
  mask.querySelector(".mhead .mt").textContent = "使用帮助";
  const copyBtn = mask.querySelector("#help-copy");
  if (copyBtn) copyBtn.onclick = () => copyText(davUrl, "WebDAV 地址已复制");
}
