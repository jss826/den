// Den - Clipboard utilities (fallback for non-secure contexts)
// eslint-disable-next-line no-unused-vars
const DenClipboard = (() => {
  async function write(text) {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return;
    }
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.cssText = 'position:fixed;left:-9999px;top:-9999px;opacity:0';
    document.body.appendChild(ta);
    ta.select();
    try { document.execCommand('copy'); } finally { ta.remove(); }
  }

  async function read() {
    if (navigator.clipboard && window.isSecureContext) {
      return await navigator.clipboard.readText();
    }
    return await Toast.prompt('Paste text:');
  }

  return { write, read };
})();
