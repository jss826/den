import { test, expect } from '@playwright/test';
import { login, getToken, filerApi } from './helpers';
import * as path from 'path';

const TEST_DIR = path.resolve(__dirname, '..', '..', 'data-e2e', 'filer-ui-test');

test.describe('Filer UI', () => {
  test.beforeAll(async ({ request }) => {
    const token = await getToken(request);
    const api = filerApi(request, token);
    // テスト用ディレクトリとファイルを作成
    await api.mkdir(TEST_DIR);
    await api.write(path.join(TEST_DIR, 'hello.txt'), 'Hello World');
    await api.write(path.join(TEST_DIR, 'test.js'), 'console.log("test");');
    await api.mkdir(path.join(TEST_DIR, 'subdir'));
    await api.write(path.join(TEST_DIR, 'subdir', 'nested.txt'), 'nested file');
  });

  test.afterAll(async ({ request }) => {
    const token = await getToken(request);
    const api = filerApi(request, token);
    await api.del(TEST_DIR);
  });

  test('Files tab switches to filer pane', async ({ page }) => {
    await login(page);
    await page.click('.tab[data-tab="filer"]');

    await expect(page.locator('#filer-pane')).toBeVisible();
    await expect(page.locator('#terminal-pane')).toBeHidden();
    await expect(page.locator('#claude-pane')).toBeHidden();
    await expect(page.locator('.tab[data-tab="filer"]')).toHaveClass(/active/);
  });

  test('filer pane shows tree, toolbar, and editor area', async ({ page }) => {
    await login(page);
    await page.click('.tab[data-tab="filer"]');

    // ツリーが表示される
    await expect(page.locator('#filer-tree')).toBeVisible();
    // ツールバーが表示される
    await expect(page.locator('.filer-toolbar')).toBeVisible();
    // エディタエリアが表示される
    await expect(page.locator('#filer-editor')).toBeVisible();
    // Welcome メッセージが表示される（ファイル未選択時）
    await expect(page.locator('.filer-welcome')).toBeVisible();
  });

  test('tree loads entries from home directory', async ({ page }) => {
    await login(page);
    await page.click('.tab[data-tab="filer"]');

    // ツリーにアイテムが表示されるまで待機
    await expect(page.locator('.tree-item').first()).toBeVisible({ timeout: 10_000 });
    const items = page.locator('.tree-item');
    expect(await items.count()).toBeGreaterThan(0);
  });

  test('clicking directory expands it in tree', async ({ page }) => {
    await login(page);
    await page.click('.tab[data-tab="filer"]');

    // ツリーが読み込まれるまで待機
    await expect(page.locator('.tree-item').first()).toBeVisible({ timeout: 10_000 });

    // 最初のディレクトリアイテムを探す
    const dirItem = page.locator('.tree-item[data-is-dir="true"]').first();
    if (await dirItem.count() > 0) {
      await dirItem.click();
      // 展開後に子要素コンテナが expanded クラスを持つ
      const toggle = dirItem.locator('.tree-toggle');
      await expect(toggle).toContainText('▾');
    }
  });

  test('clicking file opens it in editor', async ({ page, request }) => {
    await login(page);
    await page.click('.tab[data-tab="filer"]');

    // テスト用ファイルのパスをブラウザに渡すため、API 経由でファイルの絶対パスを取得
    const token = await getToken(request);
    const api = filerApi(request, token);
    const listResp = await api.list(TEST_DIR);
    const listing = await listResp.json();
    const resolvedDir = listing.path; // 正規化されたパス

    // ページ上でファイルを開く（JavaScript で直接 FilerTree を操作）
    await page.evaluate(async (filePath) => {
      // @ts-expect-error global
      await FilerEditor.openFile(filePath);
    }, path.join(resolvedDir, 'hello.txt'));

    // エディタタブが表示される
    await expect(page.locator('.filer-tab').first()).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('.filer-tab').first()).toContainText('hello.txt');

    // CodeMirror エディタが表示される
    await expect(page.locator('.cm-editor')).toBeVisible();

    // Welcome メッセージが消える
    await expect(page.locator('.filer-welcome')).toHaveCount(0);
  });

  test('editor shows file content', async ({ page, request }) => {
    await login(page);
    await page.click('.tab[data-tab="filer"]');

    const token = await getToken(request);
    const api = filerApi(request, token);
    const listResp = await api.list(TEST_DIR);
    const listing = await listResp.json();
    const resolvedDir = listing.path;

    await page.evaluate(async (filePath) => {
      // @ts-expect-error global
      await FilerEditor.openFile(filePath);
    }, path.join(resolvedDir, 'hello.txt'));

    // CodeMirror の内容に "Hello World" が含まれる
    await expect(page.locator('.cm-content')).toContainText('Hello World', { timeout: 5_000 });
  });

  test('multiple files open as tabs', async ({ page, request }) => {
    await login(page);
    await page.click('.tab[data-tab="filer"]');

    const token = await getToken(request);
    const api = filerApi(request, token);
    const listResp = await api.list(TEST_DIR);
    const listing = await listResp.json();
    const resolvedDir = listing.path;

    // 2つのファイルを開く
    await page.evaluate(async (filePath) => {
      // @ts-expect-error global
      await FilerEditor.openFile(filePath);
    }, path.join(resolvedDir, 'hello.txt'));

    await page.evaluate(async (filePath) => {
      // @ts-expect-error global
      await FilerEditor.openFile(filePath);
    }, path.join(resolvedDir, 'test.js'));

    // 2つのタブが表示される
    const tabs = page.locator('.filer-tab');
    expect(await tabs.count()).toBe(2);
    await expect(tabs.nth(0)).toContainText('hello.txt');
    await expect(tabs.nth(1)).toContainText('test.js');

    // アクティブタブは最後に開いたファイル
    await expect(tabs.nth(1)).toHaveClass(/active/);
  });

  test('closing tab removes it', async ({ page, request }) => {
    await login(page);
    await page.click('.tab[data-tab="filer"]');

    const token = await getToken(request);
    const api = filerApi(request, token);
    const listResp = await api.list(TEST_DIR);
    const listing = await listResp.json();
    const resolvedDir = listing.path;

    await page.evaluate(async (filePath) => {
      // @ts-expect-error global
      await FilerEditor.openFile(filePath);
    }, path.join(resolvedDir, 'hello.txt'));

    await expect(page.locator('.filer-tab')).toHaveCount(1);

    // 閉じるボタンをクリック
    await page.click('.filer-tab-close');
    await expect(page.locator('.filer-tab')).toHaveCount(0);

    // Welcome メッセージが再表示される
    await expect(page.locator('.filer-welcome')).toBeVisible();
  });

  test('search input exists and is interactive', async ({ page }) => {
    await login(page);
    await page.click('.tab[data-tab="filer"]');

    const searchInput = page.locator('#filer-search-input');
    await expect(searchInput).toBeVisible();

    // テキスト入力可能
    await searchInput.fill('test query');
    await expect(searchInput).toHaveValue('test query');
  });

  test('toolbar buttons are visible', async ({ page }) => {
    await login(page);
    await page.click('.tab[data-tab="filer"]');

    await expect(page.locator('#filer-new-file')).toBeVisible();
    await expect(page.locator('#filer-new-folder')).toBeVisible();
    await expect(page.locator('#filer-upload')).toBeVisible();
    await expect(page.locator('#filer-refresh')).toBeVisible();
  });

  test('upload button opens upload modal', async ({ page }) => {
    await login(page);
    await page.click('.tab[data-tab="filer"]');

    await page.click('#filer-upload');
    await expect(page.locator('#filer-upload-modal')).toBeVisible();

    // キャンセルで閉じる
    await page.click('#upload-cancel');
    await expect(page.locator('#filer-upload-modal')).toBeHidden();
  });

  test('tab switching preserves filer state', async ({ page, request }) => {
    await login(page);
    await page.click('.tab[data-tab="filer"]');

    const token = await getToken(request);
    const api = filerApi(request, token);
    const listResp = await api.list(TEST_DIR);
    const listing = await listResp.json();
    const resolvedDir = listing.path;

    // ファイルを開く
    await page.evaluate(async (filePath) => {
      // @ts-expect-error global
      await FilerEditor.openFile(filePath);
    }, path.join(resolvedDir, 'hello.txt'));
    await expect(page.locator('.filer-tab')).toHaveCount(1);

    // ターミナルタブに切り替えて戻る
    await page.click('.tab[data-tab="terminal"]');
    await expect(page.locator('#filer-pane')).toBeHidden();

    await page.click('.tab[data-tab="filer"]');
    await expect(page.locator('#filer-pane')).toBeVisible();

    // タブが残っている
    await expect(page.locator('.filer-tab')).toHaveCount(1);
    await expect(page.locator('.filer-tab')).toContainText('hello.txt');
  });
});
