/* global Toast */
// Den - SFTP リモート接続管理
// eslint-disable-next-line no-unused-vars
const FilerRemote = (() => {
  let connected = false;
  let hostInfo = null; // { host, username }

  /** 現在の API ベースパスを返す */
  function getApiBase() {
    return connected ? '/api/sftp' : '/api/filer';
  }

  /** リモート接続中かどうか */
  function isRemote() {
    return connected;
  }

  /** 接続先情報を返す */
  function getInfo() {
    return { connected, host: hostInfo?.host || null, username: hostInfo?.username || null };
  }

  /** SFTP 接続 */
  async function connect(host, port, username, authType, password, keyPath) {
    const body = { host, port: port || 22, username, auth_type: authType };
    if (authType === 'password') body.password = password;
    if (authType === 'key') body.key_path = keyPath;

    const resp = await fetch('/api/sftp/connect', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });

    if (!resp.ok) {
      const err = await resp.json().catch(() => ({ error: 'Connection failed' }));
      throw new Error(err.error || 'Connection failed');
    }

    const data = await resp.json();
    connected = data.connected;
    hostInfo = { host: data.host, username: data.username };
    document.dispatchEvent(new CustomEvent('den:sftp-changed', { detail: { connected: true } }));
    return data;
  }

  /** SFTP 切断 */
  async function disconnect() {
    try {
      await fetch('/api/sftp/disconnect', {
        method: 'POST',
        credentials: 'same-origin',
      });
    } catch { /* ignore */ }
    connected = false;
    hostInfo = null;
    document.dispatchEvent(new CustomEvent('den:sftp-changed', { detail: { connected: false } }));
  }

  /** ページロード時に既存接続を復元 */
  async function checkStatus() {
    try {
      const resp = await fetch('/api/sftp/status', { credentials: 'same-origin' });
      if (!resp.ok) return;
      const data = await resp.json();
      if (data.connected) {
        connected = true;
        hostInfo = { host: data.host, username: data.username };
        document.dispatchEvent(new CustomEvent('den:sftp-changed', { detail: { connected: true } }));
      }
    } catch { /* ignore */ }
  }

  return { getApiBase, isRemote, getInfo, connect, disconnect, checkStatus };
})();
