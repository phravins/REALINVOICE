/*
 * Runs before anything paints, so the first frame is already the right theme and there is
 * no flash of the wrong colours while the app boots.
 *
 * localStorage is a paint-time cache only. Core's settings table is the authority, and
 * app.js reconciles against it as soon as the bridge is up.
 */
(function () {
  "use strict";

  var saved = null;
  try {
    saved = localStorage.getItem("ui.theme");
  } catch (err) {
    // Private mode or blocked storage: fall through to the OS preference.
  }

  var fromOs =
    window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches
      ? "light"
      : "dark";

  var theme = saved === "light" || saved === "dark" ? saved : fromOs;
  document.documentElement.setAttribute("data-theme", theme);
})();
