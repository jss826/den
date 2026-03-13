import { test, expect } from '@playwright/test';
import { login } from './helpers';
import { spawn, execSync, type ChildProcess } from 'child_process';
import * as path from 'path';
import * as fs from 'fs';
import * as https from 'https';

const DEN_ROOT = path.resolve(__dirname, '..', '..');
const DEN_BIN = path.join(DEN_ROOT, 'target', 'debug', 'den.exe');
const SECOND_PORT = 3942;
const SECOND_DATA_DIR = path.join(DEN_ROOT, 'data-e2e2');
const PASSWORD = 'test';

/** Wait until the server responds on its HTTPS port. */
async function waitForServer(port: number, timeoutMs = 60_000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      await new Promise<void>((resolve, reject) => {
        const req = https.request(
          { hostname: '127.0.0.1', port, path: '/', method: 'GET', rejectUnauthorized: false, timeout: 2000 },
          (res) => { res.resume(); resolve(); },
        );
        req.on('error', reject);
        req.on('timeout', () => { req.destroy(); reject(new Error('timeout')); });
        req.end();
      });
      return;
    } catch {
      await new Promise((r) => setTimeout(r, 500));
    }
  }
  throw new Error(`Server on port ${port} did not start within ${timeoutMs}ms`);
}

test.describe('Quick Connect to Den', () => {
  let secondServer: ChildProcess;

  test.beforeAll(async () => {
    // Ensure data dir exists
    fs.mkdirSync(SECOND_DATA_DIR, { recursive: true });

    // Start second Den instance with a separate target-dir to avoid lock conflicts
    secondServer = spawn(DEN_BIN, [], {
      cwd: DEN_ROOT,
      env: {
        ...process.env,
        DEN_PASSWORD: PASSWORD,
        DEN_PORT: String(SECOND_PORT),
        DEN_DATA_DIR: SECOND_DATA_DIR,
        DEN_BIND_ADDRESS: '127.0.0.1',
        DEN_TLS: 'true',
      },
      stdio: 'pipe',
    });

    secondServer.stderr?.on('data', (data: Buffer) => {
      const msg = data.toString();
      if (process.env.DEBUG) {
        process.stderr.write(`[den2] ${msg}`);
      }
    });

    secondServer.on('error', (err) => {
      console.error('Failed to start second Den server:', err);
    });

    await waitForServer(SECOND_PORT);
  });

  test.afterAll(async () => {
    if (secondServer && secondServer.pid && !secondServer.killed) {
      // On Windows, process.kill() does not reliably terminate child trees.
      // Use taskkill /F /T to force-kill the process and its children.
      try {
        execSync(`taskkill /F /T /PID ${secondServer.pid}`, { stdio: 'ignore' });
      } catch {
        // Process may have already exited
      }
      // Wait briefly for process to fully exit
      await new Promise<void>((resolve) => {
        if (secondServer.exitCode !== null) { resolve(); return; }
        const timeout = setTimeout(resolve, 3000);
        secondServer.on('exit', () => { clearTimeout(timeout); resolve(); });
      });
    }

    // Clean up data-e2e2 directory
    fs.rmSync(SECOND_DATA_DIR, { recursive: true, force: true });
  });

  test('quick connect to second Den and create remote session', async ({ page }) => {
    // Step 1: Login to the first Den instance
    await login(page);

    // Step 2: Open + menu and click "Quick Connect Den..."
    await page.locator('#session-new-btn').click();
    const menu = page.locator('#new-session-menu');
    await expect(menu).toBeVisible({ timeout: 3000 });
    await menu.locator('.new-session-menu-item', { hasText: 'Quick Connect Den' }).click();

    // Step 3: Fill in the connection modal
    const modal = page.locator('#den-connect-modal');
    await expect(modal).toBeVisible({ timeout: 3000 });

    await page.locator('#den-connect-url').fill(`https://localhost:${SECOND_PORT}`);
    await page.locator('#den-connect-password').fill(PASSWORD);
    await page.locator('#den-connect-submit').click();

    // Step 4: Handle TLS trust confirmation modal
    const tlsModal = page.locator('#tls-cert-modal');
    await expect(tlsModal).toBeVisible({ timeout: 10_000 });

    // Verify the TLS modal shows the correct host
    const hostEl = page.locator('#tls-cert-host');
    await expect(hostEl).toContainText(`localhost:${SECOND_PORT}`);

    // Click Trust to accept the certificate
    await page.locator('#tls-cert-trust').click();

    // Step 5: Wait for connection to succeed — modal should close and toast should appear
    await expect(modal).toBeHidden({ timeout: 15_000 });
    await expect(page.locator('.toast')).toBeVisible({ timeout: 5000 });

    // Step 6: Open + menu again — should show "Remote localhost:{port}" section
    await page.locator('#session-new-btn').click();
    await expect(menu).toBeVisible({ timeout: 3000 });

    const remoteSep = menu.locator('.new-session-menu-separator', { hasText: `Remote localhost:${SECOND_PORT}` });
    await expect(remoteSep).toBeVisible({ timeout: 5000 });

    // "New Terminal" should be present under the remote section
    const remoteNewTerminal = menu.locator('.new-session-menu-item', { hasText: 'New Terminal' });
    await expect(remoteNewTerminal).toBeVisible();

    // Step 7: Create a remote session
    await remoteNewTerminal.click();

    // Session name prompt should appear
    const promptModal = page.locator('#prompt-modal');
    await expect(promptModal).toBeVisible({ timeout: 3000 });
    await promptModal.locator('input').fill('remote-test');
    await promptModal.locator('button', { hasText: 'OK' }).click();

    // Step 8: Verify the remote session tab appears in the session bar
    // Remote session tabs have data-session="<name>" and display as "<hostPort>:<name>"
    const sessionTab = page.locator('.session-tab', { hasText: `remote-test` });
    await expect(sessionTab).toBeVisible({ timeout: 15_000 });
    await expect(sessionTab).toHaveClass(/active/);

    // Step 9: Disconnect from remote Den to clean up state for other tests
    await page.request.post('/api/remote/disconnect');
  });
});
