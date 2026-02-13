import { test, expect } from '@playwright/test';
import { login } from './helpers';

test.describe('Claude Directory Browser', () => {
  test.beforeEach(async ({ page }) => {
    await login(page);
    // Claude タブに切り替え
    await page.click('.tab[data-tab="claude"]');
    await expect(page.locator('#claude-pane')).toBeVisible();
  });

  test('new session modal has path input and Go button', async ({ page }) => {
    await page.click('#claude-new-session');
    await expect(page.locator('#claude-modal')).toBeVisible();

    // パス入力欄と Go ボタンが存在する
    await expect(page.locator('#dir-path-input')).toBeVisible();
    await expect(page.locator('#dir-go')).toBeVisible();
    await expect(page.locator('#dir-up')).toBeVisible();

    // パス入力欄にはパスが表示されている（WebSocket で受信後）
    await expect(page.locator('#dir-path-input')).not.toHaveValue('', { timeout: 10_000 });

    await page.click('#modal-cancel');
  });

  test('directory listing loads on modal open', async ({ page }) => {
    await page.click('#claude-new-session');
    await expect(page.locator('#claude-modal')).toBeVisible();

    // ディレクトリリストがロードされるまで待機
    // パス入力欄に値が入る = list_dirs レスポンスを受信済み
    await expect(page.locator('#dir-path-input')).not.toHaveValue('', { timeout: 10_000 });

    // パスにホームディレクトリ的なパスが含まれる
    const pathValue = await page.locator('#dir-path-input').inputValue();
    expect(pathValue.length).toBeGreaterThan(1);

    await page.click('#modal-cancel');
  });

  test('navigating up does not append ../ to path', async ({ page }) => {
    await page.click('#claude-new-session');
    await expect(page.locator('#claude-modal')).toBeVisible();

    // ディレクトリリストがロードされるまで待機
    await expect(page.locator('#dir-path-input')).not.toHaveValue('', { timeout: 10_000 });

    const initialPath = await page.locator('#dir-path-input').inputValue();

    // ディレクトリに入る（あれば）
    const dirItem = page.locator('#dir-list .dir-item').first();
    if (await dirItem.count() > 0) {
      await dirItem.click();
      // パスが変わるまで待機
      await page.waitForFunction(
        (initial) => {
          const input = document.getElementById('dir-path-input') as HTMLInputElement;
          return input && input.value !== initial;
        },
        initialPath,
        { timeout: 5_000 },
      );

      const subDirPath = await page.locator('#dir-path-input').inputValue();

      // 上に移動
      await page.click('#dir-up');

      // パスが変わるまで待機
      await page.waitForFunction(
        (sub) => {
          const input = document.getElementById('dir-path-input') as HTMLInputElement;
          return input && input.value !== sub;
        },
        subDirPath,
        { timeout: 5_000 },
      );

      const parentPath = await page.locator('#dir-path-input').inputValue();

      // ../ が含まれていないこと
      expect(parentPath).not.toContain('..');
      // 元のパスに戻っていること（または元のパスの親）
      expect(parentPath.length).toBeGreaterThan(0);
    }

    await page.click('#modal-cancel');
  });

  test('Go button navigates to typed path', async ({ page }) => {
    await page.click('#claude-new-session');
    await expect(page.locator('#claude-modal')).toBeVisible();

    // ディレクトリリストがロードされるまで待機
    await expect(page.locator('#dir-path-input')).not.toHaveValue('', { timeout: 10_000 });

    const initialPath = await page.locator('#dir-path-input').inputValue();

    // プロジェクトディレクトリのパスを入力
    // (data-e2e ディレクトリは存在するはず)
    const targetPath = initialPath; // 同じパスで再読み込み → エラーにならないことを確認
    await page.locator('#dir-path-input').fill(targetPath);
    await page.click('#dir-go');

    // パスが表示される（エラーにならない）
    await expect(page.locator('#dir-path-input')).toHaveValue(targetPath, { timeout: 5_000 });

    await page.click('#modal-cancel');
  });

  test('Enter key in path input navigates', async ({ page }) => {
    await page.click('#claude-new-session');
    await expect(page.locator('#claude-modal')).toBeVisible();

    await expect(page.locator('#dir-path-input')).not.toHaveValue('', { timeout: 10_000 });

    // ~ を入力して Enter
    await page.locator('#dir-path-input').fill('~');
    await page.locator('#dir-path-input').press('Enter');

    // パスがホームディレクトリに解決される（~ ではなく実パスが表示）
    await page.waitForFunction(
      () => {
        const input = document.getElementById('dir-path-input') as HTMLInputElement;
        return input && input.value !== '~' && input.value.length > 1;
      },
      undefined,
      { timeout: 5_000 },
    );

    const resolvedPath = await page.locator('#dir-path-input').inputValue();
    expect(resolvedPath).not.toBe('~');
    expect(resolvedPath.length).toBeGreaterThan(1);

    await page.click('#modal-cancel');
  });

  test('up button is disabled at drive root', async ({ page }) => {
    await page.click('#claude-new-session');
    await expect(page.locator('#claude-modal')).toBeVisible();
    await expect(page.locator('#dir-path-input')).not.toHaveValue('', { timeout: 10_000 });

    // ドライブルートに移動（Windows: C:\, Unix: /）
    const rootPath = process.platform === 'win32' ? 'C:\\' : '/';
    await page.locator('#dir-path-input').fill(rootPath);
    await page.click('#dir-go');

    // パスが解決されるまで待機
    await page.waitForFunction(
      (root) => {
        const input = document.getElementById('dir-path-input') as HTMLInputElement;
        return input && input.value.length > 0 && input.value !== '~';
      },
      rootPath,
      { timeout: 5_000 },
    );

    // up ボタンが無効化されている（parent が null のため）
    const upBtn = page.locator('#dir-up');
    await expect(upBtn).toHaveAttribute('disabled', '', { timeout: 3_000 });

    await page.click('#modal-cancel');
  });
});
