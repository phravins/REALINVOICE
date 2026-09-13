/*
 * The console's shared component layer — the counterpart to the Back-Office Web's
 * shared_components.ex, and the reason every table, badge and form control in this app
 * looks the same as every other one.
 *
 * HEEx has function components; plain JS does not, so this is the nearest honest
 * equivalent: the class strings live in one place as named constants, and the handful of
 * markup shapes used on more than one screen are functions returning HTML. A pane that
 * wants a table asks for `UI.tableHead()`, not for a remembered pile of utility classes.
 *
 * The constants are copied from QuantumBilling's shared_components.ex deliberately. When
 * one changes there it should change here — that is the whole point of porting them by
 * name rather than by eye.
 *
 * Tailwind can only emit classes it can literally see, and it scans this file. So every
 * class name here must appear as a complete literal string: never "badge-" + kind.
 */
var UI = (function () {
  "use strict";

  /* ------------------------------------------------------------------ controls */

  var FORM_CONTROL_BASE =
    "w-full rounded-field border border-base-300 bg-base-100 px-3 text-sm " +
    "text-base-content placeholder:text-base-content/40 focus:outline-none " +
    "focus:border-base-content/30 focus:ring-2 focus:ring-base-content/10 " +
    "disabled:bg-base-200 disabled:text-base-content/60";

  var C = {
    /** The solid call-to-action in a page header. */
    actionButton:
      "inline-flex h-9 items-center gap-2 rounded-field bg-primary px-3.5 text-sm " +
      "font-medium text-primary-content transition-colors hover:bg-primary/90",

    /** Its outlined companion, for toolbar controls. */
    secondaryButton:
      "inline-flex h-9 items-center gap-2 rounded-field border border-base-300 " +
      "bg-base-100 px-3 text-sm text-base-content/60 transition-colors " +
      "hover:bg-base-200 hover:text-base-content",

    /** A size down from the header buttons: these repeat above every list. */
    filterButton:
      "inline-flex h-8 items-center gap-1.5 rounded-field border border-base-300 " +
      "bg-base-100 px-2.5 text-xs text-base-content/60 transition-colors " +
      "hover:bg-base-200 hover:text-base-content",

    /** The search field beside it; leaves room for a leading icon. */
    filterInput:
      "h-8 w-full rounded-field border border-base-300 bg-base-100 pl-8 pr-3 text-xs " +
      "placeholder:text-base-content/45 focus:outline-none focus:ring-2 " +
      "focus:ring-base-content/10",

    /** The small icon-only button used inside table rows. */
    rowAction:
      "flex size-7 items-center justify-center rounded-field text-base-content/45 " +
      "transition-colors hover:bg-base-200 hover:text-base-content",

    formInput: "h-9 " + FORM_CONTROL_BASE,
    formSelect: "h-9 appearance-none pr-8 " + FORM_CONTROL_BASE,
    formError: "border-error focus:border-error focus:ring-error/15",

    /** The uppercase micro label: sidebar sections, table headers, field captions. */
    microLabel: "text-2xs font-medium uppercase tracking-wider text-base-content/45",

    /** The circular initials chip. */
    avatar: "flex size-7 items-center justify-center rounded-full text-2xs font-semibold",

    /* `[&>th]:py-1.5` sets the header's height from here rather than cell by cell:
       daisyUI's own padding is most of the row's height on a row of 10px labels, and the
       arbitrary variant outranks it because daisyUI wraps its rules in `:where()`. */
    tableHead:
      "border-b border-base-300 text-xs font-semibold uppercase tracking-wider " +
      "text-base-content/60 [&>th]:py-1.5",

    tableRow: "border-b border-base-300 text-sm last:border-0 hover:bg-base-200/60",
  };

  /* -------------------------------------------------------------------- escaping */

  /** Everything below interpolates through this. Customer names are user input. */
  function esc(value) {
    return String(value === null || value === undefined ? "" : value).replace(
      /[&<>"']/g,
      function (ch) {
        return {
          "&": "&amp;",
          "<": "&lt;",
          ">": "&gt;",
          '"': "&quot;",
          "'": "&#39;",
        }[ch];
      }
    );
  }

  /* ------------------------------------------------------------------- fragments */

  /**
   * A status pill. `tone` picks from a fixed set rather than being interpolated into a
   * class name, so Tailwind can see every variant this app can produce.
   */
  function badge(text, tone) {
    var tones = {
      neutral: "bg-base-200 text-base-content/70",
      success: "bg-success/15 text-success",
      warning: "bg-warning/15 text-warning",
      error: "bg-error/15 text-error",
      info: "bg-info/15 text-info",
    };
    var cls = tones[tone] || tones.neutral;
    return (
      '<span class="inline-flex items-center gap-1 rounded-field px-2 py-0.5 ' +
      'text-2xs font-medium ' +
      cls +
      '">' +
      esc(text) +
      "</span>"
    );
  }

  /**
   * A label/value row pair, as used by About and Users. Returns the `<dt><dd>` for one
   * row; the caller supplies the surrounding `<dl>` so a list of them is one string.
   * `valueHtml` is inserted raw, for the rare row that carries a badge — callers passing
   * user data must escape it themselves.
   */
  function row(label, value, valueHtml) {
    return (
      '<dt class="border-t border-base-300 py-3 text-sm text-base-content/60">' +
      esc(label) +
      '</dt><dd class="border-t border-base-300 py-3 text-sm text-base-content ' +
      'break-words">' +
      (valueHtml ? valueHtml : esc(value)) +
      "</dd>"
    );
  }

  /** The `<dl>` wrapper for `row()`. First row's border is suppressed by the caller. */
  function rows(inner, extraClass) {
    return (
      '<dl class="grid grid-cols-[minmax(140px,220px)_1fr] ' +
      '[&>dt:first-of-type]:border-t-0 [&>dt:first-of-type+dd]:border-t-0 ' +
      (extraClass || "") +
      '">' +
      inner +
      "</dl>"
    );
  }

  /**
   * The friendly empty state: an icon, a line saying what is missing, and a line saying
   * what to do about it. Never a bare "No results".
   */
  function emptyState(icon, title, hint) {
    return (
      '<div class="flex flex-1 flex-col items-center justify-center gap-2 px-4 py-12 ' +
      'text-center">' +
      '<span class="' +
      esc(icon) +
      ' size-8 text-base-content/25"></span>' +
      '<p class="text-sm font-medium text-base-content">' +
      esc(title) +
      "</p>" +
      (hint ? '<p class="text-xs text-base-content/60">' + hint + "</p>" : "") +
      "</div>"
    );
  }

  /** A keyboard key, as shown in the shortcut bar and the empty states. */
  function kbd(key) {
    return (
      '<kbd class="rounded-field border border-base-300 bg-base-200 px-1.5 py-0.5 ' +
      'font-mono text-2xs text-base-content/70">' +
      esc(key) +
      "</kbd>"
    );
  }

  return {
    c: C,
    esc: esc,
    badge: badge,
    row: row,
    rows: rows,
    emptyState: emptyState,
    kbd: kbd,
  };
})();
