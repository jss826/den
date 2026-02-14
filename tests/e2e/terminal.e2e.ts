import { test, expect } from '@playwright/test';
import { login } from './helpers';

test.describe('Terminal', () => {
  test('shows Connected after login', async ({ page }) => {
    await login(page);

    // Terminal tab should be active by default
    await expect(page.locator('.tab[data-tab="terminal"]')).toHaveClass(/active/);
    await expect(page.locator('#terminal-pane')).toBeVisible();

    // Wait for "Connected" text in xterm.js terminal
    // xterm renders to DOM rows with class .xterm-rows
    await expect(page.locator('#terminal-container .xterm-rows')).toContainText('Connected', {
      timeout: 15_000,
    });
  });

  test('switch to Claude tab and back', async ({ page }) => {
    await login(page);

    // Switch to Claude tab
    await page.click('.tab[data-tab="claude"]');
    await expect(page.locator('#claude-pane')).toBeVisible();
    await expect(page.locator('#terminal-pane')).toBeHidden();

    // Switch back to terminal tab
    await page.click('.tab[data-tab="terminal"]');
    await expect(page.locator('#terminal-pane')).toBeVisible();
    await expect(page.locator('#claude-pane')).toBeHidden();
  });

  test('terminal receives command output', async ({ page }) => {
    await login(page);

    // Wait for terminal to be ready
    await expect(page.locator('#terminal-container .xterm-rows')).toContainText('Connected', {
      timeout: 15_000,
    });

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
