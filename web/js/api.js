// NDisk API 封装:统一鉴权头、JSON 解析、401 处理
import { session } from "./store.js";

export async function api(method, path, body) {
  const headers = {};
  if (session.token) headers["Authorization"] = "Bearer " + session.token;
  let payload;
  if (body !== undefined) {
    headers["Content-Type"] = "application/json";
    payload = JSON.stringify(body);
  }
  const resp = await fetch(path, { method, headers, body: payload });
  const ct = resp.headers.get("content-type") || "";
  const data = ct.includes("json") ? await resp.json().catch(() => null) : await resp.text().catch(() => null);
  if (!resp.ok) {
    if (resp.status === 401 && session.token) {
      // 会话失效:清理并回登录页
      session.expire();
    }
    const e = new Error(data && data.error ? data.error : `请求失败 (${resp.status})`);
    e.status = resp.status;
    throw e;
  }
  return data;
}

/**
 * 上传分片的 XHR(需要上传进度回调)。
 * 带无进展超时:timeoutMs 内没有任何进度 / 响应即视为失败,
 * 每收到一段数据就重置计时,慢速但活跃的连接不会被误杀。
 */
export function putChunk(token, part, blob, onProgress, timeoutMs = 60000) {
  return new Promise((resolve) => {
    const xhr = new XMLHttpRequest();
    let timer = setTimeout(() => {
      try { xhr.abort(); } catch (e) {}
      resolve(false);
    }, timeoutMs);
    const finish = (ok) => {
      clearTimeout(timer);
      resolve(ok);
    };
    const arm = () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        try { xhr.abort(); } catch (e) {}
        finish(false);
      }, timeoutMs);
    };
    xhr.open("PUT", `/api/upload/part/${token}/${part}`, true);
    if (session.token) xhr.setRequestHeader("Authorization", "Bearer " + session.token);
    xhr.upload.onprogress = (ev) => {
      if (ev.lengthComputable) {
        onProgress(ev.loaded);
        arm();
      }
    };
    xhr.onload = () => finish(xhr.status === 200);
    xhr.onerror = () => finish(false);
    xhr.send(blob);
  });
}

/** 小文件直传:单请求完成(服务端原子落盘);返回 {ok, msg?} */
export function putDirect(dest, file, onProgress, timeoutMs = 120000) {
  return new Promise((resolve) => {
    const q = `share_id=${encodeURIComponent(dest.share.id)}&path=${encodeURIComponent(dest.path)}&filename=${encodeURIComponent(file.name)}`;
    const xhr = new XMLHttpRequest();
    let timer = setTimeout(() => {
      try { xhr.abort(); } catch (e) {}
      resolve({ ok: false, msg: "直传超时" });
    }, timeoutMs);
    const finish = (ok, msg) => {
      clearTimeout(timer);
      resolve(ok ? { ok: true } : { ok: false, msg: msg || "上传失败" });
    };
    const arm = () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        try { xhr.abort(); } catch (e) {}
        finish(false, "直传超时");
      }, timeoutMs);
    };
    xhr.open("POST", `/api/upload/direct?${q}`, true);
    if (session.token) xhr.setRequestHeader("Authorization", "Bearer " + session.token);
    xhr.upload.onprogress = (ev) => {
      if (ev.lengthComputable) {
        onProgress(ev.loaded);
        arm();
      }
    };
    xhr.onload = () => {
      if (xhr.status === 200) return finish(true);
      let msg = `请求失败 (${xhr.status})`;
      try { msg = JSON.parse(xhr.responseText).error || msg; } catch (e) {}
      finish(false, msg);
    };
    xhr.onerror = () => finish(false, "网络错误");
    xhr.send(file);
  });
}
