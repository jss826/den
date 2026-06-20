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
          zellij: { available: true, sessions: ['main', 'work'], aliases: {} },
          tmux: { available: false, sessions: [], aliases: {} },
        }),
      }));

    await login(page);
    await page.locator('#session-new-btn').click();

    const menu = page.locator('#new-session-menu');
    await expect(menu).toBeVisible({ timeout: 3000 });

    // Zellij backend row is shown (sessions appear as chips inside the backend row).
    const backendRow = menu.locator('.new-session-menu-backend');
    await expect(backendRow).toBeVisible();
    await expect(backendRow.locator('.backend-icon[data-backend="zellij"]')).toBeVisible();

    // Existing sessions appear as chips; the "New" (+) chip is also present.
    await expect(menu.locator('.new-session-menu-chip', { hasText: 'main' })).toBeVisible();
    await expect(menu.locator('.new-session-menu-chip', { hasText: 'work' })).toBeVisible();
    // New-session chip ("+") is rendered at the end of each backend row.
    await expect(menu.locator('.new-session-menu-chip.new-session-menu-chip-new')).toBeVisible();

    // tmux is unavailable, so no tmux backend row appears.
    await expect(menu.locator('.new-session-menu-backend .backend-icon[data-backend="tmux"]')).toHaveCount(0);

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

  // ── New: menu grouping & backend icons ──────────────────────────────────

  test('new-session menu groups by machine and shows backend icons', async ({ page }) => {
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: { available: true, sessions: ['work'], aliases: { work: 'My Work' } },
          tmux: { available: false, sessions: [], aliases: {} },
        }),
      }));

    await login(page);
    await page.locator('#session-new-btn').click();

    const menu = page.locator('#new-session-menu');
    await expect(menu).toBeVisible({ timeout: 3000 });

    // Group header "This Den (local)" should be present.
    await expect(menu.locator('.new-session-menu-group').first()).toContainText('This Den');

    // Backend row for zellij should contain the backend icon.
    const backendRow = menu.locator('.new-session-menu-backend');
    await expect(backendRow).toBeVisible();
    await expect(backendRow.locator('.backend-icon[data-backend="zellij"]')).toBeVisible();

    // The aliased session chip ("My Work") should appear.
    await expect(menu.locator('.new-session-menu-chip', { hasText: 'My Work' })).toBeVisible();

    await page.keyboard.press('Escape');
    await expect(menu).toBeHidden();
  });

  test('session chips show alias when available, raw name otherwise', async ({ page }) => {
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: {
            available: true,
            sessions: ['aliased-sess', 'plain-sess'],
            aliases: { 'aliased-sess': 'My Alias' },
          },
          tmux: { available: false, sessions: [], aliases: {} },
        }),
      }));

    await login(page);
    await page.locator('#session-new-btn').click();

    const menu = page.locator('#new-session-menu');
    await expect(menu).toBeVisible({ timeout: 3000 });

    // Aliased session shows the alias text on the chip.
    await expect(menu.locator('.new-session-menu-chip', { hasText: 'My Alias' })).toBeVisible();
    // Un-aliased session shows the raw name.
    await expect(menu.locator('.new-session-menu-chip', { hasText: 'plain-sess' })).toBeVisible();

    await page.keyboard.press('Escape');
    await expect(menu).toBeHidden();
  });

  test('"Manage sessions…" entry is present in + menu', async ({ page }) => {
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: { available: true, sessions: ['s1'], aliases: {} },
          tmux: { available: false, sessions: [], aliases: {} },
        }),
      }));

    await login(page);
    await page.locator('#session-new-btn').click();

    const menu = page.locator('#new-session-menu');
    await expect(menu).toBeVisible({ timeout: 3000 });
    await expect(menu.locator('.new-session-menu-manage')).toBeVisible();
    await expect(menu.locator('.new-session-menu-manage')).toContainText('Manage sessions');

    await page.keyboard.press('Escape');
    await expect(menu).toBeHidden();
  });
});
