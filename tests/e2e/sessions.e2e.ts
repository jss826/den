import { test, expect } from '@playwright/test';
import { login, createSession } from './helpers';

test.describe('Session Management', () => {
  test('session bar and + button are visible after login', async ({ page }) => {
    await login(page);
    await expect(page.locator('#terminal-session-bar')).toBeVisible();
    await expect(page.locator('#session-new-btn')).toBeVisible();
  });

  test('empty state shown when no sessions exist', async ({ page }) => {
    await login(page);
    // Wait for session list to finish loading (either tabs appear or empty state)
    await expect(
      page.locator('.session-tab, .terminal-empty-state').first(),
    ).toBeVisible({ timeout: 10_000 });
    // Skip if sessions already exist from prior test runs
    if (await page.locator('.session-tab').count() > 0) {
      test.skip();
      return;
    }
    await expect(page.locator('.terminal-empty-state')).toContainText('No sessions');
  });

  test('+ button opens menu with Local Terminal option', async ({ page }) => {
    await login(page);
    await page.locator('#session-new-btn').click();

    const menu = page.locator('#new-session-menu');
    await expect(menu).toBeVisible({ timeout: 3000 });
    await expect(menu.locator('.new-session-menu-item', { hasText: 'Local Terminal' })).toBeVisible();
    await expect(menu.locator('.new-session-menu-item', { hasText: 'Quick Connect Den' })).toBeVisible();

    // Dismiss menu
    await page.keyboard.press('Escape');
    await expect(menu).toBeHidden();
  });

  test('create local session via + menu', async ({ page }) => {
    await login(page);
    await createSession(page, 'test-create');

    // Session tab should be visible and active
    const tab = page.locator('.session-tab[data-session="test-create"]');
    await expect(tab).toBeVisible();
    await expect(tab).toHaveClass(/active/);

    // Empty state should be gone
    await expect(page.locator('.terminal-empty-state')).toBeHidden();
  });

  test('create multiple sessions and switch between them', async ({ page }) => {
    await login(page);
    const ts = Date.now();
    const nameA = `sa-${ts}`;
    const nameB = `sb-${ts}`;
    await createSession(page, nameA);
    await createSession(page, nameB);

    // Both tabs should exist
    await expect(page.locator(`.session-tab[data-session="${nameA}"]`)).toBeVisible();
    await expect(page.locator(`.session-tab[data-session="${nameB}"]`)).toBeVisible();

    // nameB should be active (most recently created)
    await expect(page.locator(`.session-tab[data-session="${nameB}"]`)).toHaveClass(/active/);

    // Click nameA to switch
    await page.locator(`.session-tab[data-session="${nameA}"]`).click();
    await expect(page.locator(`.session-tab[data-session="${nameA}"]`)).toHaveClass(/active/);
    await expect(page.locator(`.session-tab[data-session="${nameB}"]`)).not.toHaveClass(/active/);
  });

  test('rename session via double-click', async ({ page }) => {
    await login(page);
    // Use unique name to avoid collision with sessions from prior runs
    const oldName = `ren-${Date.now()}`;
    const newName = `renamed-${Date.now()}`;
    await createSession(page, oldName);

    // Double-click the tab label (not the close button) to rename
    const tab = page.locator(`.session-tab[data-session="${oldName}"] .session-tab-label`);
    await tab.dblclick();

    // Prompt modal should appear
    const promptModal = page.locator('#prompt-modal');
    await expect(promptModal).toBeVisible({ timeout: 3000 });

    // Clear and enter new name
    const input = promptModal.locator('input');
    await input.clear();
    await input.fill(newName);
    await promptModal.locator('button', { hasText: 'OK' }).click();

    // New tab should appear, old one should be gone (wait for session list refresh)
    await expect(page.locator(`.session-tab[data-session="${newName}"]`)).toBeVisible({ timeout: 10_000 });
    await expect(page.locator(`.session-tab[data-session="${oldName}"]`)).toHaveCount(0, { timeout: 10_000 });
  });

  test('kill session via close button', async ({ page }) => {
    await login(page);
    await createSession(page, 'to-kill');
    await expect(page.locator('.session-tab[data-session="to-kill"]')).toBeVisible();

    // Click close button on the tab
    await page.locator('.session-tab[data-session="to-kill"] .session-tab-close').click();

    // Confirm dialog should appear
    const confirmModal = page.locator('#confirm-modal');
    await expect(confirmModal).toBeVisible({ timeout: 3000 });
    await confirmModal.locator('button', { hasText: 'OK' }).click();

    // Tab should be removed
    await expect(page.locator('.session-tab[data-session="to-kill"]')).toHaveCount(0, { timeout: 5000 });
  });

  test('invalid session name is rejected', async ({ page }) => {
    await login(page);
    await page.locator('#session-new-btn').click();
    const menu = page.locator('#new-session-menu');
    await expect(menu).toBeVisible({ timeout: 3000 });
    await menu.locator('.new-session-menu-item', { hasText: 'Local Terminal' }).click();

    const promptModal = page.locator('#prompt-modal');
    await expect(promptModal).toBeVisible({ timeout: 3000 });

    // Enter invalid name (e.g. with spaces)
    await promptModal.locator('input').fill('bad name!');
    await promptModal.locator('button', { hasText: 'OK' }).click();

    // Error toast should appear
    await expect(page.locator('.toast')).toBeVisible({ timeout: 3000 });
  });
});
