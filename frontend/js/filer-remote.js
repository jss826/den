// Den - Remote file source management (SFTP + Remote Den + Relay)
// eslint-disable-next-line no-unused-vars
const FilerRemote = (() => {
  // mode: 'local' | 'sftp' | 'den'
  let mode = 'local';
  let hostInfo = null; // { host, username } for SFTP
  let denConnections = {}; // connectionId → { type: 'direct'|'relay', url, hostPort, fingerprint, displayName, relayHostPort? }
  let activeDenId = null; // current active Den connection for filer browsing

  /** Resolve display name from trusted TLS certs cache */
  async function resolveDisplayName(hostPort) {
    if (!hostPort || typeof DenTlsTrust === 'undefined') return null;
    try {
      const certs = await DenTlsTrust.list();
      return certs[hostPort]?.display_name || null;
    } catch { return null; }
  }

  /** Current API base path */
  function getApiBase() {
    if (mode === 'den' && activeDenId) {
      const conn = denConnections[activeDenId];
      if (conn?.type === 'relay') return `/api/relay/${activeDenId}/filer`;
      return `/api/remote/${activeDenId}/filer`;
    }
    if (mode === 'sftp') return '/api/sftp';
    return '/api/filer';
  }

  /** Whether browsing a remote source */
  function isRemote() {
    return mode !== 'local';
  }

  /** Current connection info */
  function getInfo() {
    if (mode === 'den' && activeDenId && denConnections[activeDenId]) {
      const conn = denConnections[activeDenId];
      return {
        connected: true,
        mode: 'den',
        connectionId: activeDenId,
        connectionType: conn.type, // 'direct' or 'relay'
        url: conn.url || null,
        hostPort: conn.hostPort || null,
        fingerprint: conn.fingerprint || null,
        displayName: conn.displayName || null,
        relayHostPort: conn.relayHostPort || null,
        host: null,
        username: null,
      };
    }
    if (mode === 'sftp') {
      return { connected: true, mode: 'sftp', host: hostInfo?.host || null, username: hostInfo?.username || null };
    }
    return { connected: false, mode: 'local', host: null, username: null };
  }

  /** Connect to another Den over HTTPS/WSS (supports multiple simultaneous connections) */
  async function connectDen(url, password) {
    if (mode === 'sftp') await disconnectSftpSilent();

    let resp = await doDenConnectFetch(url, password);
    if (resp.status === 409) {
      const errData = await resp.json().catch(() => ({}));
      if (errData.host_port && errData.fingerprint
          && (errData.error === 'untrusted_tls_certificate' || errData.error === 'tls_fingerprint_mismatch')) {
        const accepted = await DenTlsTrust.confirmAndStore({
          hostPort: errData.host_port,
          fingerprint: errData.fingerprint,
          expectedFingerprint: errData.expected_fingerprint || null,
        });
        if (!accepted) throw new Error('Connection cancelled');
        resp = await doDenConnectFetch(url, password);
      }
    }

    if (!resp.ok) {
      const err = await resp.json().catch(() => ({ error: 'Connection failed' }));
      throw new Error(err.error || 'Connection failed');
    }

    const data = await resp.json();
    const connectionId = data.connection_id;
    const connInfo = {
      type: 'direct',
      url: data.url || url,
      hostPort: data.host_port || null,
      fingerprint: data.fingerprint || null,
      displayName: null,
    };
    connInfo.displayName = await resolveDisplayName(connInfo.hostPort);
    denConnections[connectionId] = connInfo;
    activeDenId = connectionId;
    mode = 'den';
    document.dispatchEvent(new CustomEvent('den:remote-changed', {
      detail: { mode: 'den', connectionId, hostPort: connInfo.hostPort },
    }));
    return data;
  }

  function doDenConnectFetch(url, password) {
    return fetch('/api/remote/connect', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ url, password }),
    });
  }

  /** Disconnect all Den connections silently (no event dispatch) */
  async function disconnectAllDenSilent() {
    const ids = Object.keys(denConnections);
    await Promise.all(ids.map(id => {
      const conn = denConnections[id];
      const path = conn?.type === 'relay'
        ? `/api/relay/${id}/disconnect`
        : `/api/remote/${id}/disconnect`;
      return fetch(path, { method: 'POST', credentials: 'same-origin' }).catch(() => {});
    }));
    denConnections = {};
    activeDenId = null;
    if (mode === 'den') mode = 'local';
  }

  /** Disconnect a specific Den connection (direct or relay) */
  async function disconnectDen(connectionId) {
    const id = connectionId || activeDenId;
    if (!id) return;

    const conn = denConnections[id];
    const path = conn?.type === 'relay'
      ? `/api/relay/${id}/disconnect`
      : `/api/remote/${id}/disconnect`;

    try {
      await fetch(path, { method: 'POST', credentials: 'same-origin' });
    } catch { /* ignore */ }

    delete denConnections[id];
    if (activeDenId === id) {
      const remaining = Object.keys(denConnections);
      activeDenId = remaining.length > 0 ? remaining[0] : null;
    }
    if (Object.keys(denConnections).length === 0 && mode === 'den') {
      mode = 'local';
    }
    document.dispatchEvent(new CustomEvent('den:remote-changed', {
      detail: Object.keys(denConnections).length > 0
        ? { mode: 'den', connectionId: activeDenId }
        : { mode: 'local' },
    }));
  }

  /** Connect to a target Den through a relay Den */
  async function connectDenViaRelay(relayUrl, relayPassword, targetUrl, targetPassword) {
    if (mode === 'sftp') await disconnectSftpSilent();

    let resp = await doRelayConnectFetch(relayUrl, relayPassword, targetUrl, targetPassword);

    // Handle TLS trust for relay or target (409 with hop field)
    if (resp.status === 409) {
      const errData = await resp.json().catch(() => ({}));
      if (errData.host_port && errData.fingerprint
          && (errData.error === 'untrusted_tls_certificate' || errData.error === 'tls_fingerprint_mismatch')) {
        const accepted = await DenTlsTrust.confirmAndStore({
          hostPort: errData.host_port,
          fingerprint: errData.fingerprint,
          expectedFingerprint: errData.expected_fingerprint || null,
          hop: errData.hop || null,
        });
        if (!accepted) throw new Error('Connection cancelled');

        // Retry — if target hop, include trusted_fingerprint
        const trustedFp = errData.hop === 'target' ? errData.fingerprint : null;
        resp = await doRelayConnectFetch(relayUrl, relayPassword, targetUrl, targetPassword, trustedFp);

        // Second hop might need trust too
        if (resp.status === 409) {
          const errData2 = await resp.json().catch(() => ({}));
          if (errData2.host_port && errData2.fingerprint
              && (errData2.error === 'untrusted_tls_certificate' || errData2.error === 'tls_fingerprint_mismatch')) {
            const accepted2 = await DenTlsTrust.confirmAndStore({
              hostPort: errData2.host_port,
              fingerprint: errData2.fingerprint,
              expectedFingerprint: errData2.expected_fingerprint || null,
              hop: errData2.hop || null,
            });
            if (!accepted2) throw new Error('Connection cancelled');
            const trustedFp2 = errData2.hop === 'target' ? errData2.fingerprint : null;
            resp = await doRelayConnectFetch(relayUrl, relayPassword, targetUrl, targetPassword, trustedFp2 || trustedFp);
          }
        }
      }
    }

    if (!resp.ok) {
      const err = await resp.json().catch(() => ({ error: 'Connection failed' }));
      throw new Error(err.error || 'Connection failed');
    }

    const data = await resp.json();
    const relaySessionId = data.relay_session_id;
    const targetHostPort = data.target_host_port || null;
    const displayName = await resolveDisplayName(targetHostPort);
    denConnections[relaySessionId] = {
      type: 'relay',
      hostPort: targetHostPort,
      fingerprint: data.target_fingerprint || null,
      displayName,
      relayHostPort: data.relay_host_port || null,
    };
    activeDenId = relaySessionId;
    mode = 'den';
    document.dispatchEvent(new CustomEvent('den:remote-changed', {
      detail: { mode: 'den', connectionId: relaySessionId, hostPort: targetHostPort },
    }));
    return data;
  }

  function doRelayConnectFetch(relayUrl, relayPassword, targetUrl, targetPassword, trustedFingerprint) {
    const body = {
      url: targetUrl,
      password: targetPassword,
      relay_url: relayUrl,
      relay_password: relayPassword,
    };
    if (trustedFingerprint) body.trusted_fingerprint = trustedFingerprint;
    return fetch('/api/relay/connect', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
  }

  /** SFTP connect */
  async function connect(host, port, username, authType, password, keyPath) {
    if (mode === 'den') await disconnectAllDenSilent();
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

  /** Restore connection on page load */
  async function checkStatus() {
    // Fetch relay + direct connections and TLS certs in parallel
    const [relayResult, directResult, certs] = await Promise.all([
      fetch('/api/relay/connections', { credentials: 'same-origin' }).then(r => r.ok ? r.json() : []).catch(() => []),
      fetch('/api/remote/connections', { credentials: 'same-origin' }).then(r => r.ok ? r.json() : []).catch(() => []),
      (typeof DenTlsTrust !== 'undefined' ? DenTlsTrust.list() : Promise.resolve({})).catch(() => ({})),
    ]);

    // Rebuild from API response so stale/disconnected entries are removed
    const fresh = {};

    for (const rc of relayResult) {
      fresh[rc.relay_session_id] = {
        type: 'relay',
        hostPort: rc.target_host_port || null,
        fingerprint: rc.target_fingerprint || null,
        displayName: certs[rc.target_host_port]?.display_name || null,
        relayHostPort: rc.relay_host_port || null,
      };
    }

    for (const conn of directResult) {
      fresh[conn.connection_id] = {
        type: 'direct',
        url: conn.url || null,
        hostPort: conn.host_port || null,
        fingerprint: conn.fingerprint || null,
        displayName: certs[conn.host_port]?.display_name || null,
      };
    }

    denConnections = fresh;

    // If we found any den connections, set mode
    const denIds = Object.keys(denConnections);
    if (denIds.length > 0) {
      // Preserve activeDenId if still valid, otherwise pick first
      if (!activeDenId || !denConnections[activeDenId]) {
        activeDenId = denIds[0];
      }
      mode = 'den';
      document.dispatchEvent(new CustomEvent('den:remote-changed', {
        detail: { mode: 'den', connectionId: activeDenId, hostPort: denConnections[activeDenId].hostPort },
      }));
      return;
    }

    // Check SFTP
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

  /** Refresh Den connections from backend (removes stale entries) */
  async function refreshDenConnections() {
    const [relayResult, directResult, certs] = await Promise.all([
      fetch('/api/relay/connections', { credentials: 'same-origin' }).then(r => r.ok ? r.json() : []).catch(() => []),
      fetch('/api/remote/connections', { credentials: 'same-origin' }).then(r => r.ok ? r.json() : []).catch(() => []),
      (typeof DenTlsTrust !== 'undefined' ? DenTlsTrust.list() : Promise.resolve({})).catch(() => ({})),
    ]);
    const fresh = {};
    for (const rc of relayResult) {
      fresh[rc.relay_session_id] = {
        type: 'relay',
        hostPort: rc.target_host_port || null,
        fingerprint: rc.target_fingerprint || null,
        displayName: certs[rc.target_host_port]?.display_name || null,
        relayHostPort: rc.relay_host_port || null,
      };
    }
    for (const conn of directResult) {
      fresh[conn.connection_id] = {
        type: 'direct',
        url: conn.url || null,
        hostPort: conn.host_port || null,
        fingerprint: conn.fingerprint || null,
        displayName: certs[conn.host_port]?.display_name || null,
      };
    }
    denConnections = fresh;
    if (activeDenId && !denConnections[activeDenId]) {
      const ids = Object.keys(denConnections);
      activeDenId = ids.length > 0 ? ids[0] : null;
      if (!activeDenId && mode === 'den') mode = 'local';
    }
  }

  /** Get all Den connections (copy) */
  function getDenConnections() {
    return { ...denConnections };
  }

  /** Get the active Den connection ID */
  function getActiveDenId() {
    return activeDenId;
  }

  /** Set the active Den connection for filer browsing */
  function setActiveDen(id) {
    if (!denConnections[id]) return;
    activeDenId = id;
    mode = 'den';
    document.dispatchEvent(new CustomEvent('den:remote-changed', {
      detail: { mode: 'den', connectionId: id, hostPort: denConnections[id].hostPort },
    }));
  }

  /** Switch filer to local mode */
  function switchToLocal() {
    if (mode === 'local') return;
    mode = 'local';
    document.dispatchEvent(new CustomEvent('den:remote-changed', { detail: { mode: 'local' } }));
  }

  return {
    getApiBase,
    isRemote,
    getInfo,
    connect,
    disconnect,
    connectDen,
    disconnectDen,
    connectDenViaRelay,
    checkStatus,
    getDenConnections,
    refreshDenConnections,
    getActiveDenId,
    setActiveDen,
    switchToLocal,
  };
})();
