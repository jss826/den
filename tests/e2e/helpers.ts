import { type Page, expect } from '@playwright/test';

/** Log in with the given password and wait for the main screen. */
export async function login(page: Page, password = 'e2e-test-pass') {
  await page.goto('/');
  await page.fill('#password-input', password);
  await page.click('#login-form button[type="submit"]');
  await expect(page.locator('#main-screen')).not.toHaveAttribute('hidden', { timeout: 10_000 });
}
