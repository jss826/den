// Den - TLS trust helpers for self-signed remote certificates
// eslint-disable-next-line no-unused-vars
const DenTlsTrust = (() => {
  let cache = null;

  function isValidFingerprint(value) {
    return /^SHA256:[0-9a-fA-F]{64}$/.test(value);
  }

  async function list(force) {
    if (cache && !force) return { ...cache };
    const resp = await fetch('/api/system/tls/trusted', { credentials: 'same-origin' });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    cache = await resp.json();
    return { ...cache };
  }

  async function save(hostPort, fingerprint) {
    const resp = await fetch('/api/system/tls/trusted', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ host_port: hostPort, fingerprint }),
    });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    cache = null;
  }

  async function remove(hostPort) {
    const resp = await fetch(`/api/system/tls/trusted?host_port=${encodeURIComponent(hostPort)}`, {
      method: 'DELETE',
      credentials: 'same-origin',
    });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    cache = null;
  }

  function showConfirm({ hostPort, fingerprint, expectedFingerprint, hop }) {
    return new Promise((resolve) => {
      const modal = document.getElementById('tls-cert-modal');
      const title = document.getElementById('tls-cert-title');
      const warning = document.getElementById('tls-cert-warning');
      const hostEl = document.getElementById('tls-cert-host');
      const fingerprintEl = document.getElementById('tls-cert-fingerprint');
      const expectedSection = document.getElementById('tls-cert-expected-section');
      const expectedEl = document.getElementById('tls-cert-expected');
      const cancelBtn = document.getElementById('tls-cert-cancel');
      const trustBtn = document.getElementById('tls-cert-trust');

      const isMismatch = !!expectedFingerprint && expectedFingerprint !== fingerprint;
      const hopLabel = hop === 'relay' ? 'Relay ' : hop === 'target' ? 'Target ' : '';
      title.textContent = isMismatch
        ? `${hopLabel}TLS Certificate Changed!`
        : `Trust ${hopLabel}TLS Certificate`;
      warning.hidden = !isMismatch;
      hostEl.textContent = hostPort;
      fingerprintEl.textContent = fingerprint;

      if (isMismatch) {
        expectedSection.hidden = false;
        expectedEl.textContent = expectedFingerprint;
        trustBtn.textContent = 'Update Trust';
      } else {
        expectedSection.hidden = true;
        trustBtn.textContent = 'Trust';
      }

      modal.hidden = false;
      cancelBtn.focus();

      function cleanup(result) {
        modal.hidden = true;
        cancelBtn.removeEventListener('click', onCancel);
        trustBtn.removeEventListener('click', onTrust);
        document.removeEventListener('keydown', onKeydown);
        resolve(result);
      }

      function onCancel() { cleanup(false); }
      function onTrust() { cleanup(true); }
      function onKeydown(e) {
        if (e.key === 'Escape') {
          e.preventDefault();
          onCancel();
        }
      }

      cancelBtn.addEventListener('click', onCancel);
      trustBtn.addEventListener('click', onTrust);
      document.addEventListener('keydown', onKeydown);
    });
  }

  async function confirmAndStore({ hostPort, fingerprint, expectedFingerprint, hop }) {
    if (!hostPort) throw new Error('host:port is required');
    if (!isValidFingerprint(fingerprint)) {
      throw new Error('Fingerprint must be SHA256: followed by 64 hex characters');
    }

    const accepted = await showConfirm({ hostPort, fingerprint, expectedFingerprint, hop });
    if (!accepted) return false;
    // For relay target hop, trust is passed ephemerally via trusted_fingerprint — no need to save locally
    if (hop !== 'target') {
      await save(hostPort, fingerprint);
    }
    return true;
  }

  return {
    isValidFingerprint,
    list,
    save,
    remove,
    showConfirm,
    confirmAndStore,
  };
})();
