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
