// Den - Remote file source management (SFTP + Remote Den + Relay)
// eslint-disable-next-line no-unused-vars
const FilerRemote = (() => {
  // mode: 'local' | 'sftp' | 'den' | 'relay'
  let mode = 'local';
  let hostInfo = null; // { host, username } for SFTP
  let denConnections = {}; // connectionId → { url, hostPort, fingerprint, displayName }
  let activeDenId = null; // current active Den connection for filer browsing
  let relayInfo = null; // { relaySessionId, relayHostPort, targetHostPort, targetFingerprint } for Relay

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
    if (mode === 'relay') return `/api/relay/${relayInfo.relaySessionId}/filer`;
    if (mode === 'den' && activeDenId) return `/api/remote/${activeDenId}/filer`;
    if (mode === 'sftp') return '/api/sftp';
    return '/api/filer';
  }

  /** Whether browsing a remote source */
  function isRemote() {
    return mode !== 'local';
  }

  /** Current connection info */
  function getInfo() {
    if (mode === 'relay') {
      return {
        connected: true,
        mode: 'relay',
        relaySessionId: relayInfo?.relaySessionId || null,
        relayHostPort: relayInfo?.relayHostPort || null,
        hostPort: relayInfo?.targetHostPort || null,
        fingerprint: relayInfo?.targetFingerprint || null,
        url: null,
        host: null,
        username: null,
      };
    }
    if (mode === 'den' && activeDenId && denConnections[activeDenId]) {
      const conn = denConnections[activeDenId];
      return {
        connected: true,
        mode: 'den',
        connectionId: activeDenId,
        url: conn.url || null,
        hostPort: conn.hostPort || null,
        fingerprint: conn.fingerprint || null,
        displayName: conn.displayName || null,
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
    if (mode === 'relay') await disconnectRelaySilent();

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
    await Promise.all(ids.map(id =>
      fetch(`/api/remote/${id}/disconnect`, { method: 'POST', credentials: 'same-origin' }).catch(() => {})
    ));
    denConnections = {};
    activeDenId = null;
    if (mode === 'den') mode = 'local';
  }

  /** Disconnect a specific Den connection */
  async function disconnectDen(connectionId) {
    // If no connectionId provided, disconnect the active one
    const id = connectionId || activeDenId;
    if (!id) return;

    try {
      await fetch(`/api/remote/${id}/disconnect`, {
        method: 'POST',
        credentials: 'same-origin',
      });
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
    if (mode === 'den') await disconnectAllDenSilent();
    if (mode === 'relay') await disconnectRelaySilent();

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
    mode = 'relay';
    relayInfo = {
      relaySessionId: data.relay_session_id,
      relayHostPort: data.relay_host_port || null,
      targetHostPort: data.target_host_port || null,
      targetFingerprint: data.target_fingerprint || null,
    };
    document.dispatchEvent(new CustomEvent('den:remote-changed', {
      detail: { mode: 'relay', hostPort: relayInfo.targetHostPort, relayHostPort: relayInfo.relayHostPort },
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

  async function disconnectRelaySilent() {
    if (relayInfo?.relaySessionId) {
      await fetch(`/api/relay/${relayInfo.relaySessionId}/disconnect`, {
        method: 'POST', credentials: 'same-origin',
      }).catch(() => {});
    }
    mode = 'local';
    relayInfo = null;
  }

  async function disconnectRelay() {
    if (relayInfo?.relaySessionId) {
      try {
        await fetch(`/api/relay/${relayInfo.relaySessionId}/disconnect`, {
          method: 'POST', credentials: 'same-origin',
        });
      } catch { /* ignore */ }
    }
    mode = 'local';
    relayInfo = null;
    document.dispatchEvent(new CustomEvent('den:remote-changed', { detail: { mode: 'local' } }));
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
    // Check relay status first
    try {
      const resp = await fetch('/api/relay/status', { credentials: 'same-origin' });
      if (resp.ok) {
        const data = await resp.json();
        if (data.connected) {
          mode = 'relay';
          relayInfo = {
            relaySessionId: data.relay_session_id,
            relayHostPort: data.relay_host_port || null,
            targetHostPort: data.target_host_port || null,
            targetFingerprint: data.target_fingerprint || null,
          };
          document.dispatchEvent(new CustomEvent('den:remote-changed', {
            detail: { mode: 'relay', hostPort: relayInfo.targetHostPort },
          }));
          return;
        }
      }
    } catch { /* ignore */ }
    // Check direct Den connections (multi-connection)
    try {
      const resp = await fetch('/api/remote/connections', { credentials: 'same-origin' });
      if (resp.ok) {
        const connections = await resp.json();
        if (connections.length > 0) {
          denConnections = {};
          // Resolve display names in parallel
          const hostPorts = connections.map(c => c.host_port);
          let displayNames = {};
          try {
            const certs = typeof DenTlsTrust !== 'undefined' ? await DenTlsTrust.list() : {};
            for (const hp of hostPorts) {
              displayNames[hp] = certs[hp]?.display_name || null;
            }
          } catch { /* ignore */ }
          for (const conn of connections) {
            denConnections[conn.connection_id] = {
              url: conn.url || null,
              hostPort: conn.host_port || null,
              fingerprint: conn.fingerprint || null,
              displayName: displayNames[conn.host_port] || null,
            };
          }
          activeDenId = connections[0].connection_id;
          mode = 'den';
          document.dispatchEvent(new CustomEvent('den:remote-changed', {
            detail: { mode: 'den', connectionId: activeDenId, hostPort: denConnections[activeDenId].hostPort },
          }));
          return;
        }
      }
    } catch { /* ignore */ }
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

  return {
    getApiBase,
    isRemote,
    getInfo,
    connect,
    disconnect,
    connectDen,
    disconnectDen,
    connectDenViaRelay,
    disconnectRelay,
    checkStatus,
    getDenConnections,
    getActiveDenId,
    setActiveDen,
  };
})();
