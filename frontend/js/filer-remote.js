// Den - Remote file source management (SFTP + Peer)
// eslint-disable-next-line no-unused-vars
const FilerRemote = (() => {
  // mode: 'local' | 'sftp' | 'peer'
  let mode = 'local';
  let hostInfo = null; // { host, username } for SFTP
  let peerName = null; // peer name when mode === 'peer'

  /** Current API base path */
  function getApiBase() {
    if (mode === 'peer') return `/api/peers/${encodeURIComponent(peerName)}/filer`;
    if (mode === 'sftp') return '/api/sftp';
    return '/api/filer';
  }

  /** Whether browsing a remote source */
  function isRemote() {
    return mode !== 'local';
  }

  /** Current connection info */
  function getInfo() {
    if (mode === 'peer') {
      return { connected: true, mode: 'peer', peerName, host: null, username: null };
    }
    if (mode === 'sftp') {
      return { connected: true, mode: 'sftp', peerName: null, host: hostInfo?.host || null, username: hostInfo?.username || null };
    }
    return { connected: false, mode: 'local', peerName: null, host: null, username: null };
  }

  /** Connect to a peer's file system */
  async function connectPeer(name) {
    if (mode === 'sftp') await disconnectSftpSilent();
    mode = 'peer';
    peerName = name;
    document.dispatchEvent(new CustomEvent('den:remote-changed', { detail: { mode: 'peer', peerName: name } }));
  }

  /** Disconnect from peer (back to local) */
  function disconnectPeer() {
    mode = 'local';
    peerName = null;
    document.dispatchEvent(new CustomEvent('den:remote-changed', { detail: { mode: 'local' } }));
  }

  /** SFTP connect */
  async function connect(host, port, username, authType, password, keyPath) {
    if (mode === 'peer') disconnectPeer();
    const body = { host, port: port || 22, username, auth_type: authType };
    if (authType === 'password') body.password = password;
    if (authType === 'key') body.key_path = keyPath;

    const resp = await doConnectFetch(body);

    // Handle host key verification (409 Conflict)
    if (resp.status === 409) {
      const errData = await resp.json().catch(() => ({}));
      if (errData.host_key && (errData.error === 'unknown_host_key' || errData.error === 'host_key_mismatch')) {
        const isMismatch = errData.error === 'host_key_mismatch';
        const accepted = await showHostKeyConfirm(errData.host_key, isMismatch);
        if (!accepted) throw new Error('Connection cancelled');

        // Trust the host key
        const trustResp = await fetch('/api/sftp/known-hosts', {
          method: 'POST',
          credentials: 'same-origin',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            host_port: errData.host_key.host_port,
            fingerprint: errData.host_key.fingerprint,
            algorithm: errData.host_key.algorithm,
          }),
        });
        if (!trustResp.ok) {
          const trustErr = await trustResp.json().catch(() => ({ error: 'Failed to save host key' }));
          throw new Error(trustErr.error || 'Failed to save host key');
        }

        // Retry connect (once)
        const retryResp = await doConnectFetch(body);
        if (!retryResp.ok) {
          const retryErr = await retryResp.json().catch(() => ({ error: 'Connection failed' }));
          throw new Error(retryErr.error || 'Connection failed');
        }
        const data = await retryResp.json();
        mode = 'sftp';
        hostInfo = { host: data.host, username: data.username };
        document.dispatchEvent(new CustomEvent('den:remote-changed', { detail: { mode: 'sftp' } }));
        return data;
      }
      throw new Error(errData.error || 'Connection failed');
    }

    if (!resp.ok) {
      const err = await resp.json().catch(() => ({ error: 'Connection failed' }));
      throw new Error(err.error || 'Connection failed');
    }

    const data = await resp.json();
    mode = 'sftp';
    hostInfo = { host: data.host, username: data.username };
    document.dispatchEvent(new CustomEvent('den:remote-changed', { detail: { mode: 'sftp' } }));
    return data;
  }

  /** Low-level connect fetch */
  function doConnectFetch(body) {
    return fetch('/api/sftp/connect', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
  }

  /** Show host key confirmation modal. Returns Promise<boolean>. */
  function showHostKeyConfirm(hostKey, isMismatch) {
    return new Promise((resolve) => {
      const modal = document.getElementById('hostkey-modal');
      const title = document.getElementById('hostkey-title');
      const warning = document.getElementById('hostkey-warning');
      const hostEl = document.getElementById('hostkey-host');
      const algorithmEl = document.getElementById('hostkey-algorithm');
      const fingerprintEl = document.getElementById('hostkey-fingerprint');
      const expectedSection = document.getElementById('hostkey-expected-section');
      const expectedEl = document.getElementById('hostkey-expected');
      const cancelBtn = document.getElementById('hostkey-cancel');
      const trustBtn = document.getElementById('hostkey-trust');

      title.textContent = isMismatch ? 'Host Key Changed!' : 'Unknown Host Key';
      warning.hidden = !isMismatch;
      hostEl.textContent = hostKey.host_port;
      algorithmEl.textContent = hostKey.algorithm;
      fingerprintEl.textContent = hostKey.fingerprint;

      if (isMismatch && hostKey.expected_fingerprint) {
        expectedSection.hidden = false;
        expectedEl.textContent = hostKey.expected_fingerprint;
        trustBtn.textContent = 'Update Key';
      } else {
        expectedSection.hidden = true;
        trustBtn.textContent = 'Trust';
      }

      modal.hidden = false;
      cancelBtn.focus();

      function cleanup() {
        modal.hidden = true;
        cancelBtn.removeEventListener('click', onCancel);
        trustBtn.removeEventListener('click', onTrust);
        document.removeEventListener('keydown', onKeydown);
      }
      function onCancel() { cleanup(); resolve(false); }
      function onTrust() { cleanup(); resolve(true); }
      function onKeydown(e) {
        if (e.key === 'Escape') { e.preventDefault(); onCancel(); }
      }

      cancelBtn.addEventListener('click', onCancel);
      trustBtn.addEventListener('click', onTrust);
      document.addEventListener('keydown', onKeydown);
    });
  }

  /** Disconnect SFTP silently (no event, used internally) */
  async function disconnectSftpSilent() {
    await fetch('/api/sftp/disconnect', { method: 'POST', credentials: 'same-origin' }).catch(() => {});
    mode = 'local';
    hostInfo = null;
  }

  /** SFTP disconnect */
  async function disconnect() {
    try {
      await fetch('/api/sftp/disconnect', {
        method: 'POST',
        credentials: 'same-origin',
      });
    } catch { /* ignore */ }
    mode = 'local';
    hostInfo = null;
    document.dispatchEvent(new CustomEvent('den:remote-changed', { detail: { mode: 'local' } }));
  }

  /** Restore SFTP connection on page load */
  async function checkStatus() {
    try {
      const resp = await fetch('/api/sftp/status', { credentials: 'same-origin' });
      if (!resp.ok) return;
      const data = await resp.json();
      if (data.connected) {
        mode = 'sftp';
        hostInfo = { host: data.host, username: data.username };
        document.dispatchEvent(new CustomEvent('den:remote-changed', { detail: { mode: 'sftp' } }));
      }
    } catch { /* ignore */ }
  }

  return { getApiBase, isRemote, getInfo, connect, disconnect, connectPeer, disconnectPeer, checkStatus };
})();
