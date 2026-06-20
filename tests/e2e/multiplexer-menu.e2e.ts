import { test, expect } from '@playwright/test';
import { login } from './helpers';

test.describe('Multiplexer backend menu', () => {
  test('new-session menu shows available backend submenus (status mocked)', async ({ page }) => {
    // Mock the multiplexer status: zellij available with two sessions, tmux unavailable.
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: { available: true, sessions: ['main', 'work'] },
          tmux: { available: false, sessions: [] },
        }),
      }));

    await login(page);
    await page.locator('#session-new-btn').click();

    const menu = page.locator('#new-session-menu');
    await expect(menu).toBeVisible({ timeout: 3000 });

    // Zellij section header + existing sessions (attach) + new-session entry are shown.
    await expect(menu.getByText('Zellij', { exact: true })).toBeVisible();
    await expect(menu.locator('.new-session-menu-item', { hasText: 'main' })).toBeVisible();
    await expect(menu.locator('.new-session-menu-item', { hasText: 'work' })).toBeVisible();
    await expect(menu.locator('.new-session-menu-item', { hasText: 'New zellij session' })).toBeVisible();

    // tmux is unavailable, so neither its header nor a new-tmux entry appears.
    await expect(menu.getByText('tmux', { exact: true })).toHaveCount(0);
    await expect(menu.locator('.new-session-menu-item', { hasText: 'New tmux session' })).toHaveCount(0);

    await page.keyboard.press('Escape');
    await expect(menu).toBeHidden();
  });

  test('backend conflict (409) surfaces the server message as a toast', async ({ page }) => {
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: { available: false, sessions: [] },
          tmux: { available: false, sessions: [] },
        }),
      }));
    // Intercept only the create POST; let session-list GETs through.
    await page.route('**/api/terminal/sessions', (route) => {
      if (route.request().method() === 'POST') {
        return route.fulfill({
          status: 409,
          contentType: 'text/plain',
          body: "A session named 'work' already exists with a different backend",
        });
      }
      return route.continue();
    });

    await login(page);
    await page.locator('#session-new-btn').click();
    const menu = page.locator('#new-session-menu');
    await expect(menu).toBeVisible({ timeout: 3000 });
    await menu.locator('.new-session-menu-item', { hasText: 'Local Terminal' }).click();

    const promptModal = page.locator('#prompt-modal');
    await expect(promptModal).toBeVisible({ timeout: 3000 });
    await promptModal.locator('input').fill('work');
    await promptModal.locator('button', { hasText: 'OK' }).click();

    await expect(page.locator('.toast-error')).toContainText('different backend', { timeout: 5000 });
  });

  test('no backend submenu when no multiplexer is available', async ({ page }) => {
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: { available: false, sessions: [] },
          tmux: { available: false, sessions: [] },
        }),
      }));

    await login(page);
    await page.locator('#session-new-btn').click();

    const menu = page.locator('#new-session-menu');
    await expect(menu).toBeVisible({ timeout: 3000 });

    // Local Terminal stays, but no backend sections.
    await expect(menu.locator('.new-session-menu-item', { hasText: 'Local Terminal' })).toBeVisible();
    await expect(menu.getByText('Zellij', { exact: true })).toHaveCount(0);
    await expect(menu.getByText('tmux', { exact: true })).toHaveCount(0);

    await page.keyboard.press('Escape');
    await expect(menu).toBeHidden();
  });
});
