import { test, expect } from '@playwright/test';
import { getToken, filerApi } from './helpers';
import * as path from 'path';

// テスト用ディレクトリ（プロジェクトルート配下）
const TEST_DIR = path.resolve(__dirname, '..', '..', 'data-e2e', 'filer-test');

test.describe('Filer API', () => {
  // 各テストで request を取得して API を使う
  async function setup(request: Parameters<typeof getToken>[0]) {
    const token = await getToken(request);
    return filerApi(request, token);
  }

  // 全テストの前にテストディレクトリを作成
  test('setup: create test directory', async ({ request }) => {
    const api = await setup(request);
    const resp = await api.mkdir(TEST_DIR);
    expect(resp.status()).toBeLessThan(500);
  });

  test('list: home directory returns entries', async ({ request }) => {
    const api = await setup(request);
    const resp = await api.list('~');
    expect(resp.ok()).toBe(true);
    const data = await resp.json();
    expect(data.path).toBeTruthy();
    expect(Array.isArray(data.entries)).toBe(true);
    expect(data.entries.length).toBeGreaterThan(0);
  });

  test('list: project directory has known files', async ({ request }) => {
    const api = await setup(request);
    const projectDir = path.resolve(__dirname, '..', '..');
    const resp = await api.list(projectDir);
    expect(resp.ok()).toBe(true);
    const data = await resp.json();
    const names = data.entries.map((e: { name: string }) => e.name);
    expect(names).toContain('Cargo.toml');
    expect(names).toContain('src');
    expect(names).toContain('frontend');
  });

  test('list: nonexistent path returns error', async ({ request }) => {
    const api = await setup(request);
    const resp = await api.list(path.join(TEST_DIR, 'nonexistent-dir-xyz'));
    expect(resp.ok()).toBe(false);
  });

  test('list: entries have expected fields', async ({ request }) => {
    const api = await setup(request);
    // テストファイルを作成
    await api.write(path.join(TEST_DIR, 'field-test.txt'), 'hello');
    await api.mkdir(path.join(TEST_DIR, 'field-test-dir'));

    const resp = await api.list(TEST_DIR);
    expect(resp.ok()).toBe(true);
    const data = await resp.json();

    for (const entry of data.entries) {
      expect(typeof entry.name).toBe('string');
      expect(typeof entry.is_dir).toBe('boolean');
      expect(typeof entry.size).toBe('number');
    }

    // ディレクトリが先にソートされている
    const dirs = data.entries.filter((e: { is_dir: boolean }) => e.is_dir);
    const files = data.entries.filter((e: { is_dir: boolean }) => !e.is_dir);
    if (dirs.length > 0 && files.length > 0) {
      const lastDirIdx = data.entries.lastIndexOf(dirs[dirs.length - 1]);
      const firstFileIdx = data.entries.indexOf(files[0]);
      expect(firstFileIdx).toBeGreaterThan(lastDirIdx);
    }

    // クリーンアップ
    await api.del(path.join(TEST_DIR, 'field-test.txt'));
    await api.del(path.join(TEST_DIR, 'field-test-dir'));
  });

  test('write + read: roundtrip', async ({ request }) => {
    const api = await setup(request);
    const filePath = path.join(TEST_DIR, 'roundtrip.txt');
    const content = 'Hello, World!\nLine 2\n日本語テスト';

    const writeResp = await api.write(filePath, content);
    expect(writeResp.ok()).toBe(true);

    const readResp = await api.read(filePath);
    expect(readResp.ok()).toBe(true);
    const data = await readResp.json();
    expect(data.content).toBe(content);
    expect(data.is_binary).toBe(false);
    expect(data.size).toBeGreaterThan(0);

    await api.del(filePath);
  });

  test('write: creates parent directories', async ({ request }) => {
    const api = await setup(request);
    const filePath = path.join(TEST_DIR, 'nested', 'deep', 'file.txt');
    const writeResp = await api.write(filePath, 'nested content');
    expect(writeResp.ok()).toBe(true);

    const readResp = await api.read(filePath);
    expect(readResp.ok()).toBe(true);
    const data = await readResp.json();
    expect(data.content).toBe('nested content');

    await api.del(path.join(TEST_DIR, 'nested'));
  });

  test('read: nonexistent file returns 404', async ({ request }) => {
    const api = await setup(request);
    const resp = await api.read(path.join(TEST_DIR, 'nonexistent.txt'));
    expect(resp.status()).toBe(404);
  });

  test('mkdir: creates directory', async ({ request }) => {
    const api = await setup(request);
    const dirPath = path.join(TEST_DIR, 'new-dir');
    const resp = await api.mkdir(dirPath);
    expect(resp.status()).toBe(201);

    const listResp = await api.list(TEST_DIR);
    const data = await listResp.json();
    const names = data.entries.map((e: { name: string }) => e.name);
    expect(names).toContain('new-dir');

    await api.del(dirPath);
  });

  test('rename: renames file', async ({ request }) => {
    const api = await setup(request);
    const oldPath = path.join(TEST_DIR, 'old-name.txt');
    const newPath = path.join(TEST_DIR, 'new-name.txt');

    await api.write(oldPath, 'rename test');
    const resp = await api.rename(oldPath, newPath);
    expect(resp.ok()).toBe(true);

    const oldResp = await api.read(oldPath);
    expect(oldResp.ok()).toBe(false);

    const newResp = await api.read(newPath);
    expect(newResp.ok()).toBe(true);
    const data = await newResp.json();
    expect(data.content).toBe('rename test');

    await api.del(newPath);
  });

  test('delete: removes file', async ({ request }) => {
    const api = await setup(request);
    const filePath = path.join(TEST_DIR, 'to-delete.txt');
    await api.write(filePath, 'delete me');

    const resp = await api.del(filePath);
    expect(resp.ok()).toBe(true);

    const readResp = await api.read(filePath);
    expect(readResp.ok()).toBe(false);
  });

  test('delete: removes directory recursively', async ({ request }) => {
    const api = await setup(request);
    const dirPath = path.join(TEST_DIR, 'dir-to-delete');
    await api.mkdir(dirPath);
    await api.write(path.join(dirPath, 'child.txt'), 'content');

    const resp = await api.del(dirPath);
    expect(resp.ok()).toBe(true);

    const listResp = await api.list(TEST_DIR);
    const data = await listResp.json();
    const names = data.entries.map((e: { name: string }) => e.name);
    expect(names).not.toContain('dir-to-delete');
  });

  test('search: finds files by name', async ({ request }) => {
    const api = await setup(request);
    await api.write(path.join(TEST_DIR, 'search-target.txt'), 'content');
    await api.write(path.join(TEST_DIR, 'other-file.txt'), 'content');

    const resp = await api.search(TEST_DIR, 'search-target');
    expect(resp.ok()).toBe(true);
    const results = await resp.json();
    expect(results.length).toBeGreaterThan(0);
    expect(results[0].path).toContain('search-target');

    await api.del(path.join(TEST_DIR, 'search-target.txt'));
    await api.del(path.join(TEST_DIR, 'other-file.txt'));
  });

  test('search: finds content in files', async ({ request }) => {
    const api = await setup(request);
    await api.write(path.join(TEST_DIR, 'haystack.txt'), 'The quick brown fox jumps over the lazy dog');

    const resp = await api.search(TEST_DIR, 'brown fox', true);
    expect(resp.ok()).toBe(true);
    const results = await resp.json();
    const match = results.find((r: { path: string }) => r.path.includes('haystack.txt'));
    expect(match).toBeTruthy();
    expect(match.line).toBe(1);
    expect(match.context).toContain('brown fox');

    await api.del(path.join(TEST_DIR, 'haystack.txt'));
  });

  test('download: returns file with correct headers', async ({ request }) => {
    const api = await setup(request);
    const filePath = path.join(TEST_DIR, 'download-test.txt');
    await api.write(filePath, 'download content');

    const resp = await api.download(filePath);
    expect(resp.ok()).toBe(true);
    expect(resp.headers()['content-disposition']).toContain('download-test.txt');
    const body = await resp.text();
    expect(body).toBe('download content');

    await api.del(filePath);
  });

  test('empty path is rejected', async ({ request }) => {
    const api = await setup(request);
    // 空パスでリクエスト（クエリパラメータなし → デシリアライズエラー）
    const resp = await api.read('');
    expect(resp.ok()).toBe(false);
  });

  // 最後にテストディレクトリをクリーンアップ
  test('cleanup: remove test directory', async ({ request }) => {
    const api = await setup(request);
    await api.del(TEST_DIR);
  });
});
