// Den - セッション履歴モジュール
const SessionHistory = (() => {
  const PAGE_SIZE = 20;
  let sessions = [];
  let hasMore = false;
  let onReplay = null; // (sessionMeta, events[]) => void

  async function load(append) {
    try {
      const offset = append ? sessions.length : 0;
      const resp = await fetch(`/api/sessions?offset=${offset}&limit=${PAGE_SIZE}`, {
        headers: { 'Authorization': `Bearer ${Auth.getToken()}` },
      });
      if (resp.ok) {
        const data = await resp.json();
        if (append) {
          sessions = sessions.concat(data);
        } else {
          sessions = data;
        }
        hasMore = data.length >= PAGE_SIZE;
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

  async function deleteSession(id) {
    if (!(await Toast.confirm('Delete this session?'))) return;
    try {
      const resp = await fetch(`/api/sessions/${id}`, {
        method: 'DELETE',
        headers: { 'Authorization': `Bearer ${Auth.getToken()}` },
      });
      if (resp.ok) {
        sessions = sessions.filter(s => s.id !== id);
        Toast.success('Session deleted');
        return true;
      } else if (resp.status === 409) {
        Toast.error('Cannot delete a running session');
      } else {
        Toast.error('Failed to delete session');
      }
    } catch {
      Toast.error('Failed to delete session');
    }
    return false;
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
      const statusIcon = s.status === 'completed' ? '\u2713' : s.status === 'running' ? '\u25CF' : '\u2717';

      div.innerHTML = `
        <div class="history-header">
          <span class="history-status">${statusIcon}</span>
          <span class="history-dir" title="${esc(s.working_dir || '').replace(/"/g, '&quot;')}">${esc(shortDir)}</span>
          <span class="history-date">${dateStr}</span>
        </div>
        <div class="history-prompt">${esc(shortPrompt)}</div>
        <div class="history-meta">${esc(connLabel)}</div>`;

      // Delete button
      const delBtn = document.createElement('button');
      delBtn.className = 'history-delete-btn';
      delBtn.textContent = '\u00d7';
      delBtn.title = 'Delete session';
      delBtn.addEventListener('click', async (e) => {
        e.stopPropagation();
        if (await deleteSession(s.id)) {
          render(container);
        }
      });
      div.appendChild(delBtn);

      div.addEventListener('click', () => replaySession(s.id));
      container.appendChild(div);
    }

    if (hasMore) {
      const btn = document.createElement('button');
      btn.className = 'history-load-more';
      btn.textContent = 'Load more...';
      btn.addEventListener('click', async (e) => {
        e.stopPropagation();
        await load(true);
        render(container);
      });
      container.appendChild(btn);
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

  const ESC_RE = /[&<>"']/g;
  const ESC_MAP = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
  function esc(str) {
    if (!str) return '';
    return str.replace(ESC_RE, c => ESC_MAP[c]);
  }

  function getSessions() {
    return sessions;
  }

  return { load, render, setReplayCallback, replaySession, getSessions };
})();
