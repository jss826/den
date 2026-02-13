import { type Page, type APIRequestContext, expect } from '@playwright/test';

const BASE_URL = 'http://localhost:3940';
const PASSWORD = 'e2e-test-pass';

/** Log in with the given password and wait for the main screen. */
export async function login(page: Page, password = PASSWORD) {
  await page.goto('/');
  await page.fill('#password-input', password);
  await page.click('#login-form button[type="submit"]');
  await expect(page.locator('#main-screen')).not.toHaveAttribute('hidden', { timeout: 10_000 });
}

/** Get an auth token via API. */
export async function getToken(request: APIRequestContext): Promise<string> {
  const resp = await request.post(`${BASE_URL}/api/login`, {
    data: { password: PASSWORD },
  });
  const body = await resp.json();
  return body.token;
}

/** Authenticated API helper for filer operations. */
export function filerApi(request: APIRequestContext, token: string) {
  const headers = { Authorization: `Bearer ${token}` };

  return {
    async list(path: string, showHidden = false) {
      const resp = await request.get(`${BASE_URL}/api/filer/list`, {
        headers,
        params: { path, show_hidden: String(showHidden) },
      });
      return resp;
    },

    async read(path: string) {
      const resp = await request.get(`${BASE_URL}/api/filer/read`, {
        headers,
        params: { path },
      });
      return resp;
    },

    async write(path: string, content: string) {
      const resp = await request.put(`${BASE_URL}/api/filer/write`, {
        headers,
        data: { path, content },
      });
      return resp;
    },

    async mkdir(path: string) {
      const resp = await request.post(`${BASE_URL}/api/filer/mkdir`, {
        headers,
        data: { path },
      });
      return resp;
    },

    async rename(from: string, to: string) {
      const resp = await request.post(`${BASE_URL}/api/filer/rename`, {
        headers,
        data: { from, to },
      });
      return resp;
    },

    async del(path: string) {
      const resp = await request.delete(`${BASE_URL}/api/filer/delete`, {
        headers,
        params: { path },
      });
      return resp;
    },

    async search(path: string, query: string, content = false) {
      const resp = await request.get(`${BASE_URL}/api/filer/search`, {
        headers,
        params: { path, query, content: String(content) },
      });
      return resp;
    },

    async download(path: string) {
      const resp = await request.get(`${BASE_URL}/api/filer/download`, {
        headers,
        params: { path },
      });
      return resp;
    },
  };
}
