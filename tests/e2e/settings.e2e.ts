import { test, expect } from '@playwright/test';
import { login } from './helpers';

test.describe('Settings', () => {
  test.beforeEach(async ({ page }) => {
    await login(page);
  });

  test('open and close settings modal', async ({ page }) => {
    await page.click('#settings-btn');
    await expect(page.locator('#settings-modal')).toBeVisible();

    await page.click('#settings-cancel');
    await expect(page.locator('#settings-modal')).toBeHidden();
  });

  test('change font size → save → persists after reload', async ({ page }) => {
    // Open settings and change font size
    await page.click('#settings-btn');
    await expect(page.locator('#settings-modal')).toBeVisible();

    await page.fill('#setting-font-size', '18');
    await page.click('#settings-save');
    await expect(page.locator('#settings-modal')).toBeHidden();

    // Reload and verify persisted value
    await page.reload();
    await page.waitForSelector('#main-screen:not([hidden]), #login-screen:not([hidden])', {
      timeout: 10_000,
    });

    // If still authenticated, check the setting
    const mainVisible = await page.locator('#main-screen').isVisible();
    if (mainVisible) {
      await page.click('#settings-btn');
      await expect(page.locator('#setting-font-size')).toHaveValue('18');
    }
  });
});
