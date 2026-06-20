import { test, expect } from '@playwright/test';
import { login } from './helpers';

/**
 * Open the Sessions modal by:
 *   1. clicking the + new-session button
 *   2. clicking "Manage sessions…" inside the dropdown
 *
 * The "Manage sessions…" entry lives inside buildNewSessionMenu, not as a
 * standalone button — so we must open the + menu first.
 */
async function openSessionsModal(page: import('@playwright/test').Page) {
  await page.locator('#session-new-btn').click();
  const menu = page.locator('#new-session-menu');
  await expect(menu).toBeVisible({ timeout: 3000 });
  await menu.locator('.new-session-menu-manage').click();
  await expect(page.locator('#sessions-modal')).toBeVisible({ timeout: 3000 });
}

test.describe('Sessions modal', () => {
  test('sessions modal lists sessions from mocked status', async ({ page }) => {
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: { available: true, sessions: ['work', 'dev'], aliases: {} },
          tmux: { available: false, sessions: [], aliases: {} },
        }),
      }));

    await login(page);
    await openSessionsModal(page);

    const modal = page.locator('#sessions-modal');
    await expect(modal).toBeVisible();

    // Both sessions should appear as rows.
    await expect(modal.locator('.sessions-row[data-name="work"]')).toBeVisible();
    await expect(modal.locator('.sessions-row[data-name="dev"]')).toBeVisible();

    // Both rows should have backend="zellij".
    await expect(modal.locator('.sessions-row[data-backend="zellij"]')).toHaveCount(2);

    // Close the modal.
    await modal.locator('#sessions-modal-close').click();
    await expect(modal).toBeHidden();
  });

  test('sessions modal shows aliases next to raw names', async ({ page }) => {
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: {
            available: true,
            sessions: ['work'],
            aliases: { work: 'My Work' },
          },
          tmux: { available: false, sessions: [], aliases: {} },
        }),
      }));

    await login(page);
    await openSessionsModal(page);

    const modal = page.locator('#sessions-modal');
    // Name element should contain both alias and raw name.
    const nameEl = modal.locator('.sessions-row[data-name="work"] .sessions-row-name');
    await expect(nameEl).toContainText('My Work');
    await expect(nameEl).toContainText('work');

    await modal.locator('#sessions-modal-close').click();
    await expect(modal).toBeHidden();
  });

  test('sessions modal shows empty state when no mux sessions exist', async ({ page }) => {
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: { available: true, sessions: [], aliases: {} },
          tmux: { available: false, sessions: [], aliases: {} },
        }),
      }));

    await login(page);
    await openSessionsModal(page);

    const modal = page.locator('#sessions-modal');
    // No session rows should be rendered.
    await expect(modal.locator('.sessions-row')).toHaveCount(0);
    // Empty state message should appear.
    await expect(modal.locator('.sessions-empty')).toBeVisible();

    await modal.locator('#sessions-modal-close').click();
    await expect(modal).toBeHidden();
  });

  test('sessions modal action buttons are rendered for each row', async ({ page }) => {
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: { available: true, sessions: ['alpha'], aliases: {} },
          tmux: { available: false, sessions: [], aliases: {} },
        }),
      }));

    await login(page);
    await openSessionsModal(page);

    const modal = page.locator('#sessions-modal');
    const row = modal.locator('.sessions-row[data-name="alpha"]');
    await expect(row).toBeVisible();

    // Rename, Copy attach, Kill, Delete (zellij-only) buttons should all be present.
    await expect(row.locator('[data-action="rename"]')).toBeVisible();
    await expect(row.locator('[data-action="copy"]')).toBeVisible();
    await expect(row.locator('[data-action="kill"]')).toBeVisible();
    // zellij sessions also get a Delete button.
    await expect(row.locator('[data-action="delete"]')).toBeVisible();

    await modal.locator('#sessions-modal-close').click();
    await expect(modal).toBeHidden();
  });

  test('rename button fires POST /api/multiplexer/rename with correct body', async ({ page }) => {
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: { available: true, sessions: ['proj'], aliases: {} },
          tmux: { available: false, sessions: [], aliases: {} },
        }),
      }));

    let renameBody: unknown = null;
    await page.route('**/api/multiplexer/rename', async (route) => {
      renameBody = route.request().postDataJSON();
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ ok: true }),
      });
    });

    await login(page);
    await openSessionsModal(page);

    const modal = page.locator('#sessions-modal');
    const row = modal.locator('.sessions-row[data-name="proj"]');
    await row.locator('[data-action="rename"]').click();

    // Toast.prompt opens #prompt-modal — fill the alias and confirm.
    const promptModal = page.locator('#prompt-modal');
    await expect(promptModal).toBeVisible({ timeout: 3000 });
    await promptModal.locator('#prompt-input').clear();
    await promptModal.locator('#prompt-input').fill('My Project');
    await promptModal.locator('#prompt-ok').click();

    // The rename endpoint should have been called with the right payload.
    await expect
      .poll(() => renameBody, { timeout: 5000 })
      .toMatchObject({ backend: 'zellij', name: 'proj', alias: 'My Project' });
  });

  test('kill button fires POST /api/multiplexer/kill after confirm', async ({ page }) => {
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: { available: true, sessions: ['old-sess'], aliases: {} },
          tmux: { available: false, sessions: [], aliases: {} },
        }),
      }));

    let killBody: unknown = null;
    await page.route('**/api/multiplexer/kill', async (route) => {
      killBody = route.request().postDataJSON();
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ ok: true }),
      });
    });

    await login(page);
    await openSessionsModal(page);

    const modal = page.locator('#sessions-modal');
    const row = modal.locator('.sessions-row[data-name="old-sess"]');
    await row.locator('[data-action="kill"]').click();

    // Toast.confirm opens #confirm-modal — click OK.
    const confirmModal = page.locator('#confirm-modal');
    await expect(confirmModal).toBeVisible({ timeout: 3000 });
    await confirmModal.locator('#confirm-ok').click();

    // The kill endpoint should have been called with the right payload.
    await expect
      .poll(() => killBody, { timeout: 5000 })
      .toMatchObject({ backend: 'zellij', name: 'old-sess' });
  });

  test('kill cancel does not call kill endpoint', async ({ page }) => {
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: { available: true, sessions: ['keep-me'], aliases: {} },
          tmux: { available: false, sessions: [], aliases: {} },
        }),
      }));

    let killCalled = false;
    await page.route('**/api/multiplexer/kill', (route) => {
      killCalled = true;
      return route.fulfill({ status: 200, body: JSON.stringify({ ok: true }) });
    });

    await login(page);
    await openSessionsModal(page);

    const modal = page.locator('#sessions-modal');
    const row = modal.locator('.sessions-row[data-name="keep-me"]');
    await row.locator('[data-action="kill"]').click();

    // Dismiss the confirm dialog with Cancel.
    const confirmModal = page.locator('#confirm-modal');
    await expect(confirmModal).toBeVisible({ timeout: 3000 });
    await confirmModal.locator('#confirm-cancel').click();

    // Session row should still be visible (no reload mock needed — kill didn't fire).
    await expect(killCalled).toBe(false);
    // Row still exists in the modal.
    await expect(row).toBeVisible();

    await modal.locator('#sessions-modal-close').click();
    await expect(modal).toBeHidden();
  });

  test('sessions modal closes via backdrop click', async ({ page }) => {
    await page.route('**/api/multiplexer/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          zellij: { available: false, sessions: [], aliases: {} },
          tmux: { available: false, sessions: [], aliases: {} },
        }),
      }));

    await login(page);
    await openSessionsModal(page);

    const modal = page.locator('#sessions-modal');
    await expect(modal).toBeVisible();

    // Click on the modal backdrop (the modal element itself, outside the content).
    await modal.click({ position: { x: 5, y: 5 } });
    await expect(modal).toBeHidden({ timeout: 2000 });
  });
});
