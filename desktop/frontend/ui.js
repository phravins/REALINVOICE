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

    /** A panel: bordered, faintly shadowed, on the page's own darker ground. */
    card: "rounded-box border border-base-300 bg-base-100 shadow-sm p-4",

    /** A page title and the line under it. */
    pageTitle: "text-xl font-semibold tracking-tight",
    pageSubtitle: "mt-1 text-sm text-base-content/60",

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


  /* ---------------------------------------------------------------------- charts

     Ported from the Back-Office Web's dashboard_components.ex. Hand-written SVG and
     divs, no charting library: these inherit the app's own tokens, which is why they do
     not look bolted on, and it is one fewer dependency on a machine that must keep
     billing with no network.

     Every tone below is written out in full for the same reason the Elixir version does
     it: Tailwind scans source text, so a class built as "stroke-" + tone is never
     emitted and the ring renders blank. */

  var STROKE = {
    strong: "stroke-base-content",
    medium: "stroke-base-content/60",
    soft: "stroke-base-content/35",
    faint: "stroke-base-content/15",
  };

  var DOT = {
    strong: "bg-base-content",
    medium: "bg-base-content/60",
    soft: "bg-base-content/35",
    faint: "bg-base-content/15",
  };

  var TONES = ["strong", "medium", "soft", "faint"];

  /**
   * A grouped vertical bar chart from `[{label, a, b}]`, where `a` and `b` stack side by
   * side in each slot — here, CGST+SGST beside IGST.
   *
   * `max` is supplied by the caller rather than derived, so the gridline labels are round
   * numbers instead of whatever the tallest bar happened to be.
   */
  function barChart(bars, max, formatTick) {
    if (!bars.length) return "";
    var ticks = [4, 3, 2, 1, 0].map(function (n) {
      return (max / 4) * n;
    });

    var gridLabels = ticks
      .map(function (t) {
        return "<span>" + esc(formatTick ? formatTick(t) : Math.round(t)) + "</span>";
      })
      .join("");

    var gridLines = ticks
      .map(function () {
        return '<div class="h-0 border-t border-base-200"></div>';
      })
      .join("");

    var columns = bars
      .map(function (bar) {
        // A zero-height bar is invisible, which reads as missing data rather than as a
        // quiet day; a hairline keeps the slot occupied.
        function h(value) {
          var pct = max > 0 ? (value / max) * 100 : 0;
          return value > 0 ? Math.max(pct, 0.5) : 0;
        }
        return (
          '<div class="flex h-full flex-1 items-end justify-center gap-1.5">' +
          '<div class="w-3 rounded-sm bg-base-content" style="height: ' + h(bar.a) + '%"></div>' +
          '<div class="w-3 rounded-sm bg-base-content/25" style="height: ' + h(bar.b) +
          '%"></div>' +
          "</div>"
        );
      })
      .join("");

    var labels = bars
      .map(function (bar) {
        return (
          '<span class="flex-1 text-center text-xs text-base-content/60">' +
          esc(bar.label) +
          "</span>"
        );
      })
      .join("");

    return (
      '<div class="flex gap-3">' +
      '<div class="flex h-64 flex-col justify-between text-xs text-base-content/45">' +
      gridLabels +
      "</div>" +
      '<div class="relative flex-1">' +
      '<div class="absolute inset-0 flex flex-col justify-between">' + gridLines + "</div>" +
      '<div class="relative flex h-64 items-end justify-between gap-6 px-2">' +
      columns +
      "</div>" +
      '<div class="mt-2 flex justify-between gap-6 px-2">' + labels + "</div>" +
      "</div></div>"
    );
  }

  /**
   * An SVG donut with the total in the middle and a legend beside it, from
   * `[{label, value}]`. Tones are assigned by rank, so the segments read as one series
   * rather than four unrelated colours.
   */
  function donutChart(segments, centreValue, centreLabel) {
    var total = segments.reduce(function (acc, seg) {
      return acc + seg.value;
    }, 0);
    if (total <= 0) return "";

    var offset = 0;
    var rings = "";
    var legend = "";

    segments.forEach(function (seg, index) {
      var tone = TONES[Math.min(index, TONES.length - 1)];
      var pct = (seg.value / total) * 100;
      rings +=
        '<circle cx="21" cy="21" r="15.9155" fill="none" stroke-width="5" class="' +
        STROKE[tone] +
        '" stroke-dasharray="' + pct + " " + (100 - pct) +
        '" stroke-dashoffset="' + -offset + '"></circle>';
      legend +=
        '<li class="flex items-center justify-between gap-4 text-sm">' +
        '<span class="flex items-center gap-2 text-base-content/60">' +
        '<span class="size-2.5 shrink-0 rounded-full ' + DOT[tone] + '"></span>' +
        esc(seg.label) +
        "</span>" +
        '<span class="whitespace-nowrap font-medium">' + esc(seg.display) +
        ' <span class="font-normal text-base-content/45">(' + pct.toFixed(1) + "%)</span>" +
        "</span></li>";
      offset += pct;
    });

    return (
      '<div class="flex flex-wrap items-center gap-6">' +
      '<div class="relative size-40 shrink-0">' +
      '<svg viewBox="0 0 42 42" class="size-40 -rotate-90">' + rings + "</svg>" +
      '<div class="absolute inset-0 flex flex-col items-center justify-center">' +
      '<span class="text-2xl font-semibold tracking-tight">' + esc(centreValue) + "</span>" +
      '<span class="text-xs text-base-content/60">' + esc(centreLabel) + "</span>" +
      "</div></div>" +
      '<ul class="min-w-48 flex-1 space-y-3">' + legend + "</ul>" +
      "</div>"
    );
  }

  /** A single figure with its label and an icon badge. */
  function statCard(icon, label, value, note) {
    return (
      '<div class="rounded-box border border-base-300 bg-base-100 p-4 shadow-sm">' +
      '<div class="mb-2.5 flex size-7 items-center justify-center rounded-field ' +
      'bg-base-200 text-base-content/60">' +
      '<span class="' + esc(icon) + ' size-3.5" aria-hidden="true"></span>' +
      "</div>" +
      '<p class="text-xs text-base-content/60">' + esc(label) + "</p>" +
      '<p class="mt-0.5 text-2xl font-semibold tracking-tight tabular-nums">' +
      esc(value) +
      "</p>" +
      (note ? '<p class="mt-1 text-xs text-base-content/45">' + esc(note) + "</p>" : "") +
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
    barChart: barChart,
    donutChart: donutChart,
    statCard: statCard,
  };
})();
