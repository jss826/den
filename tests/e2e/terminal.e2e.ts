import { test, expect } from '@playwright/test';
import { login, createSession } from './helpers';

test.describe('Terminal', () => {
  test('terminal tab is active by default', async ({ page }) => {
    await login(page);
    await expect(page.locator('.tab[data-tab="terminal"]')).toHaveClass(/active/);
    await expect(page.locator('#terminal-pane')).toBeVisible();
  });

  test('terminal connects after session creation', async ({ page }) => {
    await login(page);
    await createSession(page, 'term-test');

    // xterm.js should render
    await expect(page.locator('#terminal-container .xterm')).toBeVisible({ timeout: 10_000 });
  });

  test('switch to Files tab and back', async ({ page }) => {
    await login(page);

    // Switch to Files tab
    await page.click('.tab[data-tab="filer"]');
    await expect(page.locator('#filer-pane')).toBeVisible();
    await expect(page.locator('#terminal-pane')).toBeHidden();

    // Switch back to terminal tab
    await page.click('.tab[data-tab="terminal"]');
    await expect(page.locator('#terminal-pane')).toBeVisible();
    await expect(page.locator('#filer-pane')).toBeHidden();
  });

  test('full reconnect (no since) sends a VT snapshot frame then a binary redraw', async ({ page }) => {
    await login(page);
    await createSession(page, 'snap-e2e');
    // Give the PTY a moment to produce initial output (shell prompt) so the
    // byte ring is non-empty and the full replay includes a VT snapshot.
    await page.waitForTimeout(1500);

    // Open a raw WebSocket without a `since` parameter — the server treats its
    // absence as None → triggers full replay → emits snapshot control frame
    // immediately followed by a binary redraw frame.
    const frames = await page.evaluate(async (sessionName: string) => {
      const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
      const url = `${proto}//${location.host}/api/ws?cols=80&rows=24&session=${encodeURIComponent(sessionName)}`;
      return await new Promise<Array<{ kind: string; text?: string; len?: number }>>((resolve) => {
        const ws = new WebSocket(url);
        ws.binaryType = 'arraybuffer';
        const collected: Array<{ kind: string; text?: string; len?: number }> = [];
        ws.onmessage = (e: MessageEvent) => {
          if (typeof e.data === 'string') {
            collected.push({ kind: 'text', text: e.data as string });
          } else {
            collected.push({ kind: 'binary', len: (e.data as ArrayBuffer).byteLength });
          }
        };
        // Collect opening frames then close and resolve.
        setTimeout(() => { try { ws.close(); } catch (_) {} resolve(collected); }, 2500);
      });
    }, 'snap-e2e');

    // The server must send at least 2 frames: snapshot control then binary redraw.
    expect(frames.length).toBeGreaterThanOrEqual(2);
    expect(frames[0]).toEqual({ kind: 'text', text: '{"type":"snapshot"}' });
    expect(frames[1].kind).toBe('binary');
    // Binary frame = 8-byte seq prefix + at minimum some redraw bytes.
    expect(frames[1].len).toBeGreaterThanOrEqual(8);
  });

  // xterm.js v6 DOM interaction is unreliable in headless Chromium
  test.fixme('terminal receives command output', async ({ page }) => {
    await login(page);
    await createSession(page, 'cmd-test');

    // Wait for terminal to be ready
    await expect(page.locator('#terminal-container .xterm')).toBeVisible({ timeout: 10_000 });

    // Allow shell to initialize
    await page.waitForTimeout(2000);

    // Type 'echo hello123' and press Enter via xterm
    await page.locator('#terminal-container .xterm-helper-textarea').fill('');
    await page.locator('#terminal-container .xterm-helper-textarea').type('echo hello123\n', {
      delay: 50,
    });

    // Should see the output in terminal
    await expect(page.locator('#terminal-container .xterm-rows')).toContainText('hello123', {
      timeout: 10_000,
    });
  });
});
