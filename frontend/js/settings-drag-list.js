// Den - ドラッグ&ドロップリスト共通モジュール
const DenDragList = (() => {
  // F003: WeakSet で冪等性を管理（DOM 直接プロパティ汚染を回避）
  const delegated = new WeakSet();

  /**
   * リストにドラッグ&ドロップイベント委譲を設定（冪等）。
   * @param {HTMLElement} listEl - ドラッグ対象のリストコンテナ
   * @param {Object} opts
   * @param {string} opts.itemSelector - アイテムの CSS セレクタ
   * @param {string} opts.removeSelector - 削除ボタンの CSS セレクタ
   * @param {function(): {array: Array, render: function}} opts.getState
   *   呼び出し時点の配列と render 関数を返すコールバック
   */
  function init(listEl, opts) {
    if (!listEl || delegated.has(listEl)) return;
    delegated.add(listEl);

    const { itemSelector, removeSelector, getState } = opts;
    let currentDragOverEl = null;
    let touchStartIdx = null;
    let touchClone = null;
    let touchCurrentOverIdx = null;
    let touchTimer = null;
    let touchDragItem = null;
    // F004: touchmove の rAF スロットリング用
    let touchMoveRaf = null;

    function getItemIndex(el) {
      const item = el.closest(itemSelector);
      if (!item || item.dataset.index === undefined) return -1;
      return parseInt(item.dataset.index, 10);
    }

    function clearDragOver() {
      if (currentDragOverEl) {
        currentDragOverEl.classList.remove('drag-over');
        currentDragOverEl = null;
      }
    }

    function cleanupTouch() {
      clearTimeout(touchTimer);
      touchTimer = null;
      if (touchMoveRaf) { cancelAnimationFrame(touchMoveRaf); touchMoveRaf = null; }
      if (touchDragItem) {
        touchDragItem.classList.remove('dragging');
        touchDragItem = null;
      }
      clearDragOver();
      if (touchClone) { touchClone.remove(); touchClone = null; }
      touchStartIdx = null;
      touchCurrentOverIdx = null;
    }

    // Click delegation (remove)
    listEl.addEventListener('click', (e) => {
      const btn = e.target.closest(removeSelector);
      if (!btn) return;
      e.stopPropagation();
      const idx = getItemIndex(btn);
      if (idx < 0) return;
      const s = getState();
      s.array.splice(idx, 1);
      s.render();
    });

    // Desktop drag & drop delegation
    listEl.addEventListener('dragstart', (e) => {
      const idx = getItemIndex(e.target);
      if (idx < 0) return;
      e.dataTransfer.effectAllowed = 'move';
      e.dataTransfer.setData('text/plain', String(idx));
      e.target.closest(itemSelector).classList.add('dragging');
    });
    listEl.addEventListener('dragend', (e) => {
      const item = e.target.closest(itemSelector);
      if (item) item.classList.remove('dragging');
      clearDragOver();
    });
    listEl.addEventListener('dragover', (e) => {
      const item = e.target.closest(itemSelector);
      if (!item) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      if (currentDragOverEl !== item) {
        clearDragOver();
        item.classList.add('drag-over');
        currentDragOverEl = item;
      }
    });
    listEl.addEventListener('dragleave', (e) => {
      const item = e.target.closest(itemSelector);
      if (item && currentDragOverEl === item) clearDragOver();
    });
    listEl.addEventListener('drop', (e) => {
      e.preventDefault();
      clearDragOver();
      const toItem = e.target.closest(itemSelector);
      if (!toItem) return;
      const s = getState();
      const fromIdx = parseInt(e.dataTransfer.getData('text/plain'), 10);
      if (isNaN(fromIdx) || fromIdx < 0 || fromIdx >= s.array.length) return;
      // F002: toIdx の NaN・境界チェック
      const toIdx = parseInt(toItem.dataset.index, 10);
      if (isNaN(toIdx) || toIdx < 0 || toIdx >= s.array.length) return;
      if (fromIdx !== toIdx) {
        const moved = s.array.splice(fromIdx, 1)[0];
        s.array.splice(toIdx, 0, moved);
        s.render();
      }
    });

    // Touch drag & drop delegation
    listEl.addEventListener('touchstart', (e) => {
      if (e.target.closest(removeSelector)) return;
      const item = e.target.closest(itemSelector);
      if (!item) return;
      touchStartIdx = parseInt(item.dataset.index, 10);
      // F002: NaN ガード
      if (isNaN(touchStartIdx)) { touchStartIdx = null; return; }
      const touch = e.touches[0];
      touchTimer = setTimeout(() => {
        touchTimer = null;
        touchDragItem = item;
        item.classList.add('dragging');
        touchClone = item.cloneNode(true);
        const rect = item.getBoundingClientRect();
        touchClone.style.position = 'fixed';
        touchClone.style.zIndex = '999';
        touchClone.style.pointerEvents = 'none';
        touchClone.style.opacity = '0.8';
        touchClone.style.width = rect.width + 'px';
        touchClone.style.left = (touch.clientX - 20) + 'px';
        touchClone.style.top = (touch.clientY - 20) + 'px';
        document.body.appendChild(touchClone);
      }, 200);
    }, { passive: true });

    // F004: rAF スロットリングで touchmove の高頻度処理を抑制
    listEl.addEventListener('touchmove', (e) => {
      if (!touchClone) return;
      e.preventDefault();
      if (touchMoveRaf) return;
      const touch = e.touches[0];
      const cx = touch.clientX;
      const cy = touch.clientY;
      touchMoveRaf = requestAnimationFrame(() => {
        touchMoveRaf = null;
        if (!touchClone) return;
        touchClone.style.left = (cx - 20) + 'px';
        touchClone.style.top = (cy - 20) + 'px';
        const overEl = document.elementFromPoint(cx, cy);
        const overItem = overEl ? overEl.closest(itemSelector) : null;
        if (overItem && overItem.dataset.index !== undefined) {
          if (currentDragOverEl !== overItem) {
            clearDragOver();
            overItem.classList.add('drag-over');
            currentDragOverEl = overItem;
          }
          touchCurrentOverIdx = parseInt(overItem.dataset.index, 10);
        } else {
          clearDragOver();
          touchCurrentOverIdx = null;
        }
      });
    }, { passive: false });

    listEl.addEventListener('touchend', () => {
      const startIdx = touchStartIdx;
      const overIdx = touchCurrentOverIdx;
      const hadClone = !!touchClone;
      cleanupTouch();
      // F002: NaN・境界チェック
      if (hadClone && overIdx !== null && startIdx !== null && startIdx !== overIdx) {
        const s = getState();
        if (startIdx >= 0 && startIdx < s.array.length && overIdx >= 0 && overIdx < s.array.length) {
          const moved = s.array.splice(startIdx, 1)[0];
          s.array.splice(overIdx, 0, moved);
          s.render();
        }
      }
    });

    listEl.addEventListener('touchcancel', () => {
      cleanupTouch();
    });
  }

  return { init };
})();

// F005: Node.js テスト用エクスポート
if (typeof module !== 'undefined') module.exports = DenDragList;
