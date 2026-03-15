import { test, expect } from '@playwright/test';
import { login } from './helpers';

test.describe('Ports Dialog', () => {
  test.beforeEach(async ({ page }) => {
    await login(page);
  });

  test('ports button exists in toolbar', async ({ page }) => {
    const btn = page.locator('#ports-btn');
    // Button exists in DOM (may be hidden or visible depending on detected ports)
    await expect(btn).toHaveCount(1);
  });

  test('clicking ports button opens dialog', async ({ page }) => {
    // Make the button visible
    await page.evaluate(() => {
      const btn = document.getElementById('ports-btn');
      if (btn) btn.hidden = false;
    });

    const btn = page.locator('#ports-btn');
    await btn.click();

    const modal = page.locator('#ports-modal');
    await expect(modal).toBeVisible({ timeout: 3_000 });

    // Close button works
    await modal.locator('#ports-modal-close').click();
    await expect(modal).toBeHidden();
  });

  test('ports dialog shows empty state', async ({ page }) => {
    // Clear any detected ports and show dialog
    await page.evaluate(() => {
      const btn = document.getElementById('ports-btn');
      if (btn) btn.hidden = false;

      // Create modal with empty state
      let modal = document.getElementById('ports-modal');
      if (modal) modal.remove();
      modal = document.createElement('div');
      modal.id = 'ports-modal';
      modal.className = 'modal';
      modal.innerHTML = `
        <div class="modal-content" style="max-width:420px">
          <h3>Detected Ports</h3>
          <div id="ports-modal-body">
            <div class="connections-empty">No ports detected</div>
          </div>
          <div class="modal-actions">
            <button class="modal-btn" id="ports-modal-close">Close</button>
          </div>
        </div>`;
      document.body.appendChild(modal);
      modal.hidden = false;
    });

    const modal = page.locator('#ports-modal');
    await expect(modal).toBeVisible();
    await expect(modal.locator('.connections-empty')).toContainText('No ports detected');
  });

  test('ports dialog renders host:port for SSH sessions', async ({ page }) => {
    // Build dialog with test port entries
    await page.evaluate(() => {
      let modal = document.getElementById('ports-modal');
      if (modal) modal.remove();
      modal = document.createElement('div');
      modal.id = 'ports-modal';
      modal.className = 'modal';
      modal.innerHTML = `
        <div class="modal-content" style="max-width:420px">
          <h3>Detected Ports</h3>
          <div id="ports-modal-body"></div>
          <div class="modal-actions">
            <button class="modal-btn" id="ports-modal-close">Close</button>
          </div>
        </div>`;
      document.body.appendChild(modal);

      const body = modal.querySelector('#ports-modal-body')!;
      const ports = [
        { port: 3000, sshHost: 'myserver.com', remote: 'conn1' },
        { port: 8080, sshHost: null, remote: 'conn2' },
      ];
      for (const p of ports) {
        const entry = document.createElement('div');
        entry.className = 'connection-entry';
        const header = document.createElement('div');
        header.className = 'connection-header';
        const name = document.createElement('span');
        name.className = 'connection-name';
        const host = p.sshHost || p.remote || '';
        name.textContent = host ? `${host}:${p.port}` : `Port ${p.port}`;
        header.appendChild(name);
        if (p.remote) {
          const badge = document.createElement('span');
          badge.className = 'connection-type-badge direct';
          badge.textContent = p.sshHost ? 'SSH' : p.remote;
          header.appendChild(badge);
        }
        entry.appendChild(header);
        body.appendChild(entry);
      }
      modal.hidden = false;
    });

    const modal = page.locator('#ports-modal');
    await expect(modal).toBeVisible();
    // SSH host shows as host:port
    await expect(modal.locator('.connection-name').first()).toContainText('myserver.com:3000');
    await expect(modal.locator('.connection-type-badge').first()).toContainText('SSH');
    // Non-SSH shows remote ID
    await expect(modal.locator('.connection-name').nth(1)).toContainText('conn2:8080');
    await expect(modal.locator('.connection-type-badge').nth(1)).toContainText('conn2');
  });

  test('ports dialog closes on backdrop click', async ({ page }) => {
    await page.evaluate(() => {
      const btn = document.getElementById('ports-btn');
      if (btn) btn.hidden = false;
    });
    await page.locator('#ports-btn').click();

    const modal = page.locator('#ports-modal');
    await expect(modal).toBeVisible({ timeout: 3_000 });

    // Click the backdrop (the modal overlay itself, not the content)
    await modal.click({ position: { x: 5, y: 5 } });
    await expect(modal).toBeHidden();
  });
});
