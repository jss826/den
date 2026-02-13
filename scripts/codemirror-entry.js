// CodeMirror 6 bundle entry point
// Exports everything needed as window.CM

// Core
export {
  EditorView, keymap, highlightSpecialChars, drawSelection,
  dropCursor, highlightActiveLine, lineNumbers, highlightActiveLineGutter,
  rectangularSelection, crosshairCursor,
} from '@codemirror/view';

export { EditorState, Compartment } from '@codemirror/state';

export {
  defaultKeymap, history, historyKeymap,
  indentWithTab,
} from '@codemirror/commands';

export {
  indentOnInput, syntaxHighlighting, defaultHighlightStyle,
  bracketMatching, foldGutter, foldKeymap,
  HighlightStyle,
} from '@codemirror/language';

export { tags } from '@lezer/highlight';

export {
  autocompletion, completionKeymap, closeBrackets, closeBracketsKeymap,
} from '@codemirror/autocomplete';

export {
  searchKeymap, highlightSelectionMatches, search,
} from '@codemirror/search';

// Theme
export { oneDark, oneDarkTheme } from '@codemirror/theme-one-dark';

// Languages
export { javascript } from '@codemirror/lang-javascript';
export { html } from '@codemirror/lang-html';
export { css } from '@codemirror/lang-css';
export { json } from '@codemirror/lang-json';
export { markdown } from '@codemirror/lang-markdown';
export { rust } from '@codemirror/lang-rust';
export { python } from '@codemirror/lang-python';
export { yaml } from '@codemirror/lang-yaml';

// Legacy modes for TOML
export { StreamLanguage } from '@codemirror/language';
export { toml } from '@codemirror/legacy-modes/mode/toml';

// Convenience: basicSetup-like configuration
import { keymap, highlightSpecialChars, drawSelection, dropCursor, highlightActiveLine, lineNumbers, highlightActiveLineGutter, rectangularSelection, crosshairCursor, EditorView } from '@codemirror/view';
import { EditorState } from '@codemirror/state';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { indentOnInput, syntaxHighlighting, defaultHighlightStyle, bracketMatching, foldGutter, foldKeymap } from '@codemirror/language';
import { autocompletion, completionKeymap, closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete';
import { searchKeymap, highlightSelectionMatches } from '@codemirror/search';

export const denSetup = [
  lineNumbers(),
  highlightActiveLineGutter(),
  highlightSpecialChars(),
  history(),
  foldGutter(),
  drawSelection(),
  dropCursor(),
  EditorState.allowMultipleSelections.of(true),
  indentOnInput(),
  syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
  bracketMatching(),
  closeBrackets(),
  autocompletion(),
  rectangularSelection(),
  crosshairCursor(),
  highlightActiveLine(),
  highlightSelectionMatches(),
  keymap.of([
    ...closeBracketsKeymap,
    ...defaultKeymap,
    ...searchKeymap,
    ...historyKeymap,
    ...foldKeymap,
    ...completionKeymap,
    indentWithTab,
  ]),
];
