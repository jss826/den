// Den - セッション履歴モジュール
const SessionHistory = (() => {
  let sessions = [];
  let onReplay = null; // (sessionMeta, events[]) => void

  async function load() {
    try {
      const resp = await fetch('/api/sessions', {
        headers: { 'Authorization': `Bearer ${Auth.getToken()}` },
      });
      if (resp.ok) {
        sessions = await resp.json();
      }
    } catch (e) {
      console.warn('Failed to load session history:', e);
    }
    return sessions;
  }

  function setReplayCallback(cb) {
    onReplay = cb;
  }

  async function replaySession(id) {
    try {
      const [metaResp, eventsResp] = await Promise.all([
        fetch(`/api/sessions/${id}`, {
          headers: { 'Authorization': `Bearer ${Auth.getToken()}` },
        }),
        fetch(`/api/sessions/${id}/events`, {
          headers: { 'Authorization': `Bearer ${Auth.getToken()}` },
        }),
      ]);

      if (!metaResp.ok || !eventsResp.ok) return;

      const meta = await metaResp.json();
      const events = await eventsResp.json();

      if (onReplay) onReplay(meta, events);
    } catch (e) {
      console.warn('Failed to replay session:', e);
    }
  }

  function render(container) {
    container.innerHTML = '';

    if (sessions.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'history-empty';
      empty.textContent = 'No past sessions';
      container.appendChild(empty);
      return;
    }

    for (const s of sessions) {
      const div = document.createElement('div');
      div.className = 'history-item';

      const connLabel = s.connection?.type === 'local' ? 'Local' : (s.connection?.host || '?');
      const shortDir = (s.working_dir || '').split(/[/\\]/).pop() || s.working_dir;
      const shortPrompt = (s.prompt || '').slice(0, 40) + (s.prompt?.length > 40 ? '...' : '');
      const date = new Date(s.created_at);
      const dateStr = formatDate(date);
      const statusIcon = s.status === 'completed' ? '✓' : s.status === 'running' ? '●' : '✗';

      div.innerHTML = `
        <div class="history-header">
          <span class="history-status">${statusIcon}</span>
          <span class="history-dir">${esc(shortDir)}</span>
          <span class="history-date">${dateStr}</span>
        </div>
        <div class="history-prompt">${esc(shortPrompt)}</div>
        <div class="history-meta">${esc(connLabel)}</div>`;

      div.addEventListener('click', () => replaySession(s.id));
      container.appendChild(div);
    }
  }

  function formatDate(d) {
    const now = new Date();
    const diff = now - d;
    if (diff < 60000) return 'just now';
    if (diff < 3600000) return Math.floor(diff / 60000) + 'm ago';
    if (diff < 86400000) return Math.floor(diff / 3600000) + 'h ago';
    return d.toLocaleDateString();
  }

  function esc(str) {
    const d = document.createElement('div');
    d.textContent = str || '';
    return d.innerHTML;
  }

  function getSessions() {
    return sessions;
  }

  return { load, render, setReplayCallback, replaySession, getSessions };
})();
