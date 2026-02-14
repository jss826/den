import { test, expect } from '@playwright/test';
import { login } from './helpers';

test.describe('Session Management', () => {
  test('session bar is visible after login', async ({ page }) => {
    await login(page);

    // Terminal tab should be active by default
    await expect(page.locator('.tab[data-tab="terminal"]')).toHaveClass(/active/);

    // Session bar should be visible
    await expect(page.locator('#terminal-session-bar')).toBeVisible();

    // Session select dropdown should exist
    await expect(page.locator('#session-select')).toBeVisible();

    // New and kill buttons should be visible
    await expect(page.locator('#session-new-btn')).toBeVisible();
    await expect(page.locator('#session-kill-btn')).toBeVisible();
  });

  test('default session is selected', async ({ page }) => {
    await login(page);

    // Wait for session list to load
    await expect(page.locator('#session-select option')).toHaveCount(1, {
      timeout: 10_000,
    });

    // Default session should be selected
    const select = page.locator('#session-select');
    await expect(select).toHaveValue('default');
  });

  test('session select shows session options', async ({ page }) => {
    await login(page);

    // Wait for terminal to connect and session list to populate
    await expect(page.locator('#terminal-container .xterm-rows')).toContainText('Connected', {
      timeout: 15_000,
    });

    // There should be at least one session (default)
    const options = page.locator('#session-select option');
    expect(await options.count()).toBeGreaterThanOrEqual(1);
  });
});
