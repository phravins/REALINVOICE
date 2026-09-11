/*
 * Office Console — stage 1 frontend.
 *
 * Plain ES5-compatible DOM code, no framework and no build step: the bundle these SME
 * billing machines load is these three files and nothing else. Its whole job right now
 * is to prove the invoke() bridge into realinvoice-core works.
 */

(function () {
  "use strict";

  // withGlobalTauri is on, so invoke() is available without a module import.
  var invoke =
    window.__TAURI__ && window.__TAURI__.core
      ? window.__TAURI__.core.invoke
      : null;

  var $ = function (id) {
    return document.getElementById(id);
  };

  var status = function (text) {
    $("status-line").textContent = text;
  };

  /* ------------------------------------------------------------------ tabs */

  var tabs = Array.prototype.slice.call(document.querySelectorAll(".tab"));

  function showPane(name) {
    tabs.forEach(function (tab) {
      var active = tab.dataset.pane === name;
      tab.classList.toggle("is-active", active);
      tab.setAttribute("aria-selected", active ? "true" : "false");
    });
    Array.prototype.forEach.call(document.querySelectorAll(".pane"), function (pane) {
      pane.classList.toggle("is-active", pane.id === "pane-" + name);
    });
    status(name === "billing" ? "Ready." : name + ": coming soon.");
  }

  tabs.forEach(function (tab) {
    tab.addEventListener("click", function () {
      showPane(tab.dataset.pane);
    });
  });

  /* --------------------------------------------------------------- helpers */

  function escapeHtml(value) {
    return String(value).replace(/[&<>"']/g, function (ch) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[ch];
    });
  }

  function money(value) {
    // Indian grouping: 1,23,900.00.
    return Number(value).toLocaleString("en-IN", {
      minimumFractionDigits: 2,
      maximumFractionDigits: 2,
    });
  }

  function showError(target, err) {
    target.innerHTML =
      '<span class="error">' + escapeHtml(err && err.message ? err.message : err) + "</span>";
  }

  /* ------------------------------------------------------- title bar status */

  function loadNodeStatus() {
    if (!invoke) return;
    invoke("node_status")
      .then(function (info) {
        $("node-label").textContent = "[Node: " + info.node + "]";
        var badge = $("conn-badge");
        badge.textContent = info.connected ? "[Connected]" : "[Offline]";
        badge.className = "badge " + (info.connected ? "badge--connected" : "badge--offline");
        status("db: " + info.db_path);
      })
      .catch(function (err) {
        status("node_status failed: " + err);
      });
  }

  /* ------------------------------------------------------ customer lookup */

  function searchCustomer() {
    var target = $("customer-result");
    var mobile = $("mobile-input").value.trim();

    if (!mobile) {
      target.innerHTML = '<span class="muted">Enter a mobile number first.</span>';
      return;
    }
    if (!invoke) {
      showError(target, "Tauri bridge unavailable — run this through the desktop app.");
      return;
    }

    status("Searching " + mobile + "…");
    invoke("search_customer", { mobile: mobile })
      .then(function (customer) {
        if (!customer) {
          target.innerHTML =
            '<span class="muted">No customer found for ' + escapeHtml(mobile) + ".</span>";
          status("No match.");
          return;
        }
        target.innerHTML =
          "<dl class='kv'>" +
          "<dt>Name</dt><dd>" + escapeHtml(customer.name) + "</dd>" +
          "<dt>GSTIN</dt><dd>" + escapeHtml(customer.gstin || "— unregistered —") + "</dd>" +
          "<dt>Place of supply</dt><dd>" + escapeHtml(customer.place_of_supply) + "</dd>" +
          "<dt>Mobile</dt><dd>" + escapeHtml(customer.mobile) + "</dd>" +
          "</dl>";
        status("Matched customer #" + customer.id + ".");
      })
      .catch(function (err) {
        showError(target, err);
        status("Search failed.");
      });
  }

  /* ------------------------------------------------------ today's invoices */

  function listTodaysInvoices() {
    var target = $("invoice-result");

    if (!invoke) {
      showError(target, "Tauri bridge unavailable — run this through the desktop app.");
      return;
    }

    status("Loading today's invoices…");
    invoke("list_todays_invoices")
      .then(function (invoices) {
        if (!invoices.length) {
          target.innerHTML = '<span class="muted">No invoices raised today.</span>';
          status("0 invoices today.");
          return;
        }
        var rows = invoices
          .map(function (inv) {
            return (
              "<li>" +
              escapeHtml(inv.invoice_no) +
              " &nbsp;·&nbsp; " +
              escapeHtml(inv.date) +
              " &nbsp;·&nbsp; " +
              escapeHtml(inv.payment_type) +
              " &nbsp;·&nbsp; ₹" +
              money(inv.grand_total) +
              " &nbsp;·&nbsp; " +
              escapeHtml(inv.sync_status) +
              "</li>"
            );
          })
          .join("");
        target.innerHTML = "<ul class='list'>" + rows + "</ul>";
        status(invoices.length + " invoice(s) today.");
      })
      .catch(function (err) {
        showError(target, err);
        status("Load failed.");
      });
  }

  /* ----------------------------------------------------------------- wiring */

  $("search-btn").addEventListener("click", searchCustomer);
  $("mobile-input").addEventListener("keydown", function (event) {
    if (event.key === "Enter") searchCustomer();
  });
  $("list-btn").addEventListener("click", listTodaysInvoices);

  loadNodeStatus();
  showPane("billing");
  $("mobile-input").focus();
})();
