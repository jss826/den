// Den - ローディングスピナー
// eslint-disable-next-line no-unused-vars
const Spinner = (() => {
  const SPINNER_CLASS = 'spinner-overlay';

  /**
   * targetEl 内にスピナーオーバーレイを表示
   */
  function show(targetEl) {
    if (targetEl.querySelector('.' + SPINNER_CLASS)) return;
    const overlay = document.createElement('div');
    overlay.className = SPINNER_CLASS;
    overlay.innerHTML = '<div class="spinner-ring"></div>';
    targetEl.style.position = targetEl.style.position || 'relative';
    targetEl.appendChild(overlay);
  }

  /**
   * targetEl からスピナーオーバーレイを除去
   */
  function hide(targetEl) {
    const overlay = targetEl.querySelector('.' + SPINNER_CLASS);
    if (overlay) overlay.remove();
  }

  /**
   * 非同期処理中だけ targetEl にスピナー表示
   */
  async function wrap(targetEl, promiseFn) {
    show(targetEl);
    try {
      return await promiseFn();
    } finally {
      hide(targetEl);
    }
  }

  /**
   * ボタンを disabled にしてスピナー表示、完了後に復帰
   */
  async function button(btn, promiseFn) {
    btn.disabled = true;
    const original = btn.textContent;
    btn.classList.add('btn-loading');
    try {
      return await promiseFn();
    } finally {
      btn.disabled = false;
      btn.textContent = original;
      btn.classList.remove('btn-loading');
    }
  }

  return { show, hide, wrap, button };
})();
