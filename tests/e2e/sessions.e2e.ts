import { test, expect, type Page } from '@playwright/test';
import { login, createSession } from './helpers';

// Lexical global exposed by the terminal module (not on window — see CLAUDE.md).
declare const DenTerminal: {
  getTerminal(): { buffer: { active: { length: number; getLine(i: number): { translateToString(trim: boolean): string } | undefined } }; write(d: string, cb?: () => void): void } | null;
  getCurrentSession(): string | null;
};

/** True if the ACTIVE terminal's buffer (scrollback included) contains `marker`. */
async function activeTermContains(page: Page, marker: string): Promise<boolean> {
  return page.evaluate((m) => {
    const t = DenTerminal.getTerminal();
    if (!t) return false;
    const buf = t.buffer.active;
    for (let i = 0; i < buf.length; i++) {
      const line = buf.getLine(i);
      if (line && line.translateToString(true).includes(m)) return true;
    }
    return false;
  }, marker);
}

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
    const name = `tc-${Date.now()}`;
    await createSession(page, name);

    // Session tab should be visible and active
    const tab = page.locator(`.session-tab[data-session="${name}"]`);
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

  test('scrollback is preserved when switching sessions (#115)', async ({ page }) => {
    await login(page);
    const ts = Date.now();
    const nameA = `pa-${ts}`;
    const nameB = `pb-${ts}`;
    await createSession(page, nameA);
    await createSession(page, nameB);

    // Switch to A and wait until it is the active terminal.
    await page.locator(`.session-tab[data-session="${nameA}"]`).click();
    await expect(page.locator(`.session-tab[data-session="${nameA}"]`)).toHaveClass(/active/);
    // With per-session retention each session has its OWN host; only the active
    // host is shown. Exactly one visible .xterm must be present.
    await expect(page.locator('.term-session-host:not([hidden]) .xterm')).toBeVisible({ timeout: 10_000 });
    await page.waitForFunction((n) => DenTerminal.getCurrentSession() === n && !!DenTerminal.getTerminal(), nameA);

    // Write a client-side marker into A's buffer (not server scrollback). The old
    // shared-term implementation reset the term on switch, which would drop it.
    const marker = `MARKER-${ts}`;
    await page.evaluate((m) => new Promise<void>((resolve) => {
      DenTerminal.getTerminal()!.write(`${m}\r\n`, () => resolve());
    }), marker);
    expect(await activeTermContains(page, marker)).toBe(true);

    // Switch away to B, then back to A.
    await page.locator(`.session-tab[data-session="${nameB}"]`).click();
    await expect(page.locator(`.session-tab[data-session="${nameB}"]`)).toHaveClass(/active/);
    await page.waitForFunction((n) => DenTerminal.getCurrentSession() === n, nameB);
    await page.locator(`.session-tab[data-session="${nameA}"]`).click();
    await expect(page.locator(`.session-tab[data-session="${nameA}"]`)).toHaveClass(/active/);
    await page.waitForFunction((n) => DenTerminal.getCurrentSession() === n && !!DenTerminal.getTerminal(), nameA);

    // Per-session term retention keeps A's client-written scrollback across the
    // round-trip (#115). With the old reset-on-switch, the marker would be gone.
    await expect.poll(() => activeTermContains(page, marker), { timeout: 5000 }).toBe(true);
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
    const name = `kill-${Date.now()}`;
    await createSession(page, name);
    await expect(page.locator(`.session-tab[data-session="${name}"]`)).toBeVisible();

    // Scroll tab into view and click close button
    const closeBtn = page.locator(`.session-tab[data-session="${name}"] .session-tab-close`);
    await closeBtn.scrollIntoViewIfNeeded();
    await closeBtn.click();

    // Confirm dialog should appear
    const confirmModal = page.locator('#confirm-modal');
    await expect(confirmModal).toBeVisible({ timeout: 3000 });
    await confirmModal.locator('button', { hasText: 'OK' }).click();

    // Tab should be removed
    await expect(page.locator(`.session-tab[data-session="${name}"]`)).toHaveCount(0, { timeout: 5000 });
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
