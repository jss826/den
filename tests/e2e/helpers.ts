import { type Page, type APIRequestContext, expect } from '@playwright/test';

const PASSWORD = 'e2e-test-pass';

/** Log in with the given password and wait for the main screen. */
export async function login(page: Page, password = PASSWORD) {
  await page.goto('/');
  await page.fill('#password-input', password);
  await page.click('#login-form button[type="submit"]');
  await expect(page.locator('#main-screen')).not.toHaveAttribute('hidden', { timeout: 10_000 });
}

/**
 * Create a local terminal session via the + menu.
 * Assumes the terminal tab is already active.
 */
export async function createSession(page: Page, name: string) {
  await page.locator('#session-new-btn').click();
  const menu = page.locator('#new-session-menu');
  await expect(menu).toBeVisible({ timeout: 3000 });
  await menu.locator('.new-session-menu-item', { hasText: 'Local Terminal' }).click();

  const promptModal = page.locator('#prompt-modal');
  await expect(promptModal).toBeVisible({ timeout: 3000 });
  await promptModal.locator('input').fill(name);
  await promptModal.locator('button', { hasText: 'OK' }).click();

  await expect(page.locator(`.session-tab[data-session="${name}"]`)).toBeVisible({ timeout: 15_000 });
}

/**
 * Create an authenticated API request context.
 * Login sets HttpOnly cookies, so we use a persistent context that preserves them.
 */
export async function loginApiContext(request: APIRequestContext): Promise<APIRequestContext> {
  await request.post('/api/login', {
    data: { password: PASSWORD },
  });
  // The request context now has the auth cookies set by Set-Cookie headers
  return request;
}

/** Authenticated API helper for filer operations (uses cookie auth). */
export function filerApi(request: APIRequestContext) {

  return {
    async list(path: string, showHidden = false) {
      const resp = await request.get('/api/filer/list', {

        params: { path, show_hidden: String(showHidden) },
      });
      return resp;
    },

    async read(path: string) {
      const resp = await request.get('/api/filer/read', {

        params: { path },
      });
      return resp;
    },

    async write(path: string, content: string) {
      const resp = await request.put('/api/filer/write', {

        data: { path, content },
      });
      return resp;
    },

    async mkdir(path: string) {
      const resp = await request.post('/api/filer/mkdir', {

        data: { path },
      });
      return resp;
    },

    async rename(from: string, to: string) {
      const resp = await request.post('/api/filer/rename', {

        data: { from, to },
      });
      return resp;
    },

    async del(path: string) {
      const resp = await request.delete('/api/filer/delete', {

        params: { path },
      });
      return resp;
    },

    async search(path: string, query: string, content = false, showHidden = false) {
      const resp = await request.get('/api/filer/search', {

        params: { path, query, content: String(content), show_hidden: String(showHidden) },
      });
      return resp;
    },

    async download(path: string) {
      const resp = await request.get('/api/filer/download', {

        params: { path },
      });
      return resp;
    },
  };
}
