import { test, expect } from '@playwright/test';
import { login } from './helpers';

/**
 * Claude セッション E2E テスト（期間限定）
 *
 * 実際の Claude CLI を起動し API トークンを消費する。
 * CI には含めず、手動実行で ConPTY 出力ブロック修正を検証する:
 *   npx playwright test claude-session
 */
test.describe('Claude Session (live)', () => {
  test.setTimeout(120_000); // Claude CLI 起動 + API 応答に余裕を持たせる

  test('session produces output and completes', async ({ page }) => {
    await login(page);

    // Claude タブに切り替え
    await page.click('.tab[data-tab="claude"]');
    await expect(page.locator('#claude-pane')).toBeVisible();

    // 新規セッションモーダルを開く
    await page.click('#claude-new-session');
    await expect(page.locator('#claude-modal')).toBeVisible();

    // ディレクトリリストがロードされるまで待機
    await expect(page.locator('#dir-path-input')).not.toHaveValue('', {
      timeout: 10_000,
    });

    // 最小限のプロンプトを入力（トークン節約）
    await page.locator('#modal-prompt').fill('Reply with just the word hello');

    // セッション開始
    await page.click('#modal-start');

    // モーダルが閉じる
    await expect(page.locator('#claude-modal')).toBeHidden({ timeout: 5_000 });

    // ヘッダーが running になる（ターン開始）
    const headerStatus = page.locator('#claude-header .header-status');
    await expect(headerStatus).toHaveText('running', { timeout: 15_000 });

    // Claude の出力が表示される（= ConPTY ブロックが解消されている）
    // .msg-assistant, .msg-system, .msg-result いずれかが出現するのを待つ
    const anyMsg = page.locator(
      '#claude-messages .msg-assistant, #claude-messages .msg-system, #claude-messages .msg-result',
    );
    await expect(anyMsg.first()).toBeVisible({ timeout: 90_000 });

    // ターン完了を待つ（.msg-result が表示される = turn_completed 受信済み）
    await expect(page.locator('#claude-messages .msg-result')).toBeVisible({
      timeout: 90_000,
    });

    // ヘッダーのステータスが idle に戻る
    await expect(headerStatus).toHaveText('idle', { timeout: 10_000 });

    // メッセージが1つ以上存在する（出力ゼロではない）
    const msgCount = await page
      .locator('#claude-messages .msg')
      .count();
    expect(msgCount).toBeGreaterThanOrEqual(1);
  });
});
