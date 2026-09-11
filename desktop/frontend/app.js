/*
 * Office Console — Billing pane (stage 2).
 *
 * Plain ES5-compatible DOM code, no framework and no build step: what these SME billing
 * machines load is these three files and nothing else.
 *
 * The one rule that shapes this file: no money is computed here. Row totals, the GST
 * split and the grand total all come back from the `quote_invoice` command, which calls
 * core's GST module — the same code that will price the invoice when stage 3 saves it.
 * JavaScript only formats what core returns.
 */

(function () {
  "use strict";

  var invoke =
    window.__TAURI__ && window.__TAURI__.core ? window.__TAURI__.core.invoke : null;

  var $ = function (id) {
    return document.getElementById(id);
  };

  function status(text) {
    $("status-line").textContent = text;
  }

  function escapeHtml(value) {
    return String(value).replace(/[&<>"']/g, function (ch) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[ch];
    });
  }

  /** Indian grouping, two decimals: 1,23,900.00 */
  function money(value) {
    return Number(value).toLocaleString("en-IN", {
      minimumFractionDigits: 2,
      maximumFractionDigits: 2,
    });
  }

  function rupees(value) {
    return "₹" + money(value);
  }

  function errText(err) {
    return err && err.message ? err.message : String(err);
  }

  function bridgeMissing(where) {
    status(where + ": Tauri bridge unavailable — run this through the desktop app.");
  }

  /* --------------------------------------------------------------------- state */

  /** The resolved customer, or null. `place_of_supply` drives the GST split. */
  var customer = null;

  /** Billing rows: { item_id, item_code, description, qty, rate, tax_rate, total }. */
  var rows = [];

  /** Guards against an older quote landing after a newer one. */
  var quoteSeq = 0;

  /** Last quote returned by core, for the Print & Lock payload. */
  var lastQuote = null;

  /** True once a save succeeds. A locked screen is read-only until New Transaction. */
  var locked = false;

  /** The invoice this screen just saved, for the print preview. */
  var lastSaved = null;

  /** Rows currently listed in the History pane. */
  var historyRows = [];

  /* ----------------------------------------------- shared renderers (billing + history) */

  /**
   * The financial summary. One implementation, two callers: the billing card feeds it a
   * live quote from core, the history detail feeds it a saved invoice. Intra-state shows
   * CGST + SGST, inter-state shows IGST — never both.
   */
  function renderTotals(el, t) {
    function row(label, value, cls) {
      return (
        '<div class="tot-row' + (cls || "") + '"><dt>' + label + "</dt><dd>" +
        rupees(value) + "</dd></div>"
      );
    }

    var html = row("Subtotal", t.subtotal);
    if (t.intra_state) {
      html += row("CGST", t.cgst) + row("SGST", t.sgst);
    } else {
      html += row("IGST", t.igst);
    }
    el.innerHTML = html + row("Grand Total", t.grand_total, " tot-row--grand");
  }

  /** A saved invoice's totals, in the shape renderTotals expects. */
  function totalsOf(invoice) {
    return {
      // A stored invoice carries the split itself: IGST is only ever set inter-state.
      intra_state: !(invoice.igst > 0),
      subtotal: invoice.subtotal,
      cgst: invoice.cgst,
      sgst: invoice.sgst,
      igst: invoice.igst,
      grand_total: invoice.grand_total,
    };
  }

  /**
   * The item rows. `editable` gives each row a Qty box and a remove button for the
   * billing card; without it the same columns render as plain text for the read-only
   * detail view and the printable sheet.
   */
  function lineRowsHtml(lines, editable) {
    return lines
      .map(function (line, index) {
        var qty = editable
          ? '<input class="qty-input" type="number" min="0" step="any" value="' +
            line.qty + '" data-index="' + index + '" aria-label="Quantity for ' +
            escapeHtml(line.item_code) + '" />'
          : '<span class="mono">' + line.qty + "</span>";

        var remove = editable
          ? '<td class="num"><button class="row-del" data-remove="' + index +
            '" title="Remove row" aria-label="Remove ' + escapeHtml(line.item_code) +
            '">×</button></td>'
          : "";

        return (
          "<tr>" +
          '<td class="mono">' + escapeHtml(line.item_code) + "</td>" +
          "<td>" + escapeHtml(line.description) + "</td>" +
          '<td class="num">' + qty + "</td>" +
          '<td class="num mono">' + money(line.rate) + "</td>" +
          '<td class="num mono">' + money(line.tax_rate) + "</td>" +
          '<td class="num mono" data-total="' + index + '">' + money(line.total) + "</td>" +
          remove +
          "</tr>"
        );
      })
      .join("");
  }

  /** `YYYY-MM-DD HH:MM:SS` -> `YYYY-MM-DD HH:MM`, falling back to the invoice date. */
  function stamp(invoice) {
    var at = invoice.created_at || "";
    return at.length >= 16 ? at.slice(0, 16) : invoice.date;
  }

  /* ----------------------------------------------------------------------- tabs */

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
    if (name === "history") {
      // Reload on every visit so an invoice saved a moment ago is already listed.
      loadHistory();
    } else if (name !== "billing") {
      status(name + ": coming soon.");
    }
  }

  tabs.forEach(function (tab) {
    tab.addEventListener("click", function () {
      showPane(tab.dataset.pane);
    });
  });

  /* --------------------------------------------------------- title bar status */

  function loadNodeStatus() {
    if (!invoke) return bridgeMissing("node_status");
    invoke("node_status")
      .then(function (info) {
        $("node-label").textContent = "[Node: " + info.node + "]";
        var badge = $("conn-badge");
        badge.textContent = info.connected ? "[Connected]" : "[Offline]";
        badge.className = "badge " + (info.connected ? "badge--connected" : "badge--offline");
        status("db: " + info.db_path);
      })
      .catch(function (err) {
        status("node_status failed: " + errText(err));
      });
  }

  /* ------------------------------------------------------------------ customer */

  function renderCustomer() {
    var line = $("customer-line");
    if (!customer) {
      line.innerHTML = '<span class="muted">Search a mobile number to attach a customer.</span>';
      return;
    }
    line.innerHTML =
      '<span class="cust-name">' + escapeHtml(customer.name) + "</span>" +
      ' <span class="cust-gstin">— GSTIN: ' +
      escapeHtml(customer.gstin || "unregistered") +
      "</span>" +
      ' <span class="cust-pos">· ' + escapeHtml(customer.mobile) +
      " · place of supply " + escapeHtml(customer.place_of_supply) + "</span>";
  }

  function setCustomer(found) {
    customer = found;
    renderCustomer();
    hideNewCustomer();
    requote();
  }

  function searchCustomer() {
    var mobile = $("mobile-input").value.trim();
    if (!mobile) {
      status("Enter a mobile number first.");
      return;
    }
    if (!invoke) return bridgeMissing("search_customer");

    status("Searching " + mobile + "…");
    invoke("search_customer", { mobile: mobile })
      .then(function (found) {
        if (found) {
          setCustomer(found);
          status("Customer #" + found.id + " attached.");
        } else {
          customer = null;
          renderCustomer();
          showNewCustomer(mobile);
          status("No customer for " + mobile + " — register one below.");
        }
      })
      .catch(function (err) {
        status("search_customer failed: " + errText(err));
      });
  }

  /* -------------------------------------------------------------- new customer */

  function showNewCustomer(mobile) {
    $("nc-name").value = "";
    $("nc-mobile").value = mobile || "";
    $("nc-gstin").value = "";
    $("nc-pos").value = "TN";
    $("nc-msg").textContent = "";
    $("new-customer").hidden = false;
    $("nc-name").focus();
  }

  function hideNewCustomer() {
    $("new-customer").hidden = true;
  }

  function createCustomer() {
    var msg = $("nc-msg");
    var payload = {
      name: $("nc-name").value.trim(),
      mobile: $("nc-mobile").value.trim(),
      gstin: $("nc-gstin").value.trim() || null,
      place_of_supply: $("nc-pos").value.trim().toUpperCase(),
    };

    if (!payload.name || !payload.mobile || !payload.place_of_supply) {
      msg.className = "newcust-msg error";
      msg.textContent = "Name, mobile and place of supply are required.";
      return;
    }
    if (!invoke) return bridgeMissing("create_customer");

    msg.className = "newcust-msg muted";
    msg.textContent = "Saving…";
    invoke("create_customer", { payload: payload })
      .then(function (created) {
        $("mobile-input").value = created.mobile;
        setCustomer(created);
        status("Registered " + created.name + " (#" + created.id + ").");
      })
      .catch(function (err) {
        msg.className = "newcust-msg error";
        msg.textContent = errText(err);
      });
  }

  /* ---------------------------------------------------------------- item rows */

  function renderRows() {
    var body = $("items-body");
    $("empty-rows").hidden = rows.length > 0;
    body.innerHTML = rows.length ? lineRowsHtml(rows, !locked) : "";
  }

  function addRow(item) {
    rows.push({
      item_id: item.id,
      item_code: item.item_code,
      description: item.description,
      uom: item.uom,
      qty: 1,
      rate: item.rate,
      tax_rate: item.tax_rate,
      total: item.rate, // provisional; core's quote overwrites it
    });
    renderRows();
    requote();
    status("Added " + item.item_code + ".");
  }

  function removeRow(index) {
    var gone = rows.splice(index, 1)[0];
    renderRows();
    requote();
    status(gone ? "Removed " + gone.item_code + "." : "Row removed.");
  }

  // Delegated so the handlers survive every re-render.
  $("items-body").addEventListener("click", function (event) {
    var button = event.target.closest("[data-remove]");
    if (button) removeRow(Number(button.dataset.remove));
  });

  $("items-body").addEventListener("input", function (event) {
    var input = event.target.closest(".qty-input");
    if (!input) return;

    var qty = parseFloat(input.value);
    var valid = isFinite(qty) && qty > 0;
    input.classList.toggle("is-bad", !valid && input.value !== "");
    // A half-typed quantity prices as zero rather than throwing the totals away.
    rows[Number(input.dataset.index)].qty = valid ? qty : 0;
    requote();
  });

  /* -------------------------------------------------------------- item picker */

  function openPicker() {
    $("item-picker").hidden = false;
    $("item-query").value = "";
    runItemSearch("");
    $("item-query").focus();
  }

  function closePicker() {
    $("item-picker").hidden = true;
  }

  function runItemSearch(query) {
    var list = $("picker-results");
    if (!invoke) {
      list.innerHTML = '<li class="picker-empty error">Tauri bridge unavailable.</li>';
      return;
    }

    invoke("search_item", { query: query })
      .then(function (items) {
        if (!items.length) {
          list.innerHTML = '<li class="picker-empty">No items match.</li>';
          return;
        }
        list.innerHTML = items
          .map(function (item, index) {
            return (
              '<li><button class="picker-pick" data-pick="' + index + '">' +
              '<span class="pk-code">' + escapeHtml(item.item_code) + "</span>" +
              "<span>" + escapeHtml(item.description) + "</span>" +
              '<span class="pk-meta">₹' + money(item.rate) + " · " +
              money(item.tax_rate) + "% · " + escapeHtml(item.uom) + "</span>" +
              "</button></li>"
            );
          })
          .join("");
        list.dataset.items = JSON.stringify(items);
      })
      .catch(function (err) {
        list.innerHTML = '<li class="picker-empty error">' + escapeHtml(errText(err)) + "</li>";
      });
  }

  $("add-item-btn").addEventListener("click", openPicker);
  $("picker-close").addEventListener("click", closePicker);

  $("item-query").addEventListener("input", function (event) {
    runItemSearch(event.target.value);
  });

  $("picker-results").addEventListener("click", function (event) {
    var button = event.target.closest("[data-pick]");
    if (!button) return;
    var items = JSON.parse($("picker-results").dataset.items || "[]");
    var picked = items[Number(button.dataset.pick)];
    if (picked) {
      addRow(picked);
      closePicker();
    }
  });

  document.addEventListener("keydown", function (event) {
    if (event.key === "Escape" && !$("item-picker").hidden) closePicker();
  });

  /* ----------------------------------------------------------------- summary */

  function renderQuote(quote) {
    lastQuote = quote;

    // Row totals are core's line values, not a number this file worked out.
    quote.line_totals.forEach(function (total, index) {
      if (rows[index]) rows[index].total = total;
      var cell = document.querySelector('#items-body [data-total="' + index + '"]');
      if (cell) cell.textContent = money(total);
    });

    renderTotals($("billing-totals"), quote);

    $("gst-mode").textContent = customer
      ? (quote.intra_state ? "Intra-state" : "Inter-state") +
        " · " + quote.home_state + " → " + customer.place_of_supply
      : "No customer — totals assume intra-state " + quote.home_state;
  }

  /**
   * Clears a stale message. Anything that changes the transaction makes a previous
   * confirmation or error wrong, and a stale line next to fresh totals is how a counter
   * mis-bills. A save confirmation is exempt: the screen is locked, so nothing beneath it
   * can change.
   */
  function clearMessage() {
    if (locked) return;
    var msg = $("action-msg");
    msg.className = "action-msg";
    msg.textContent = "";
  }

  /** Re-prices every row through core. Called on any change to rows or customer. */
  function requote() {
    clearMessage();
    if (!invoke) return bridgeMissing("quote_invoice");

    // With no customer resolved yet, quote against the home state so the operator still
    // sees live figures; the split is re-quoted the moment one is attached.
    var placeOfSupply = customer ? customer.place_of_supply : "TN";
    var lines = rows.map(function (row) {
      return { qty: row.qty, rate: row.rate, tax_rate: row.tax_rate };
    });

    var seq = ++quoteSeq;
    invoke("quote_invoice", { placeOfSupply: placeOfSupply, lines: lines })
      .then(function (quote) {
        if (seq !== quoteSeq) return; // a newer edit already won
        renderQuote(quote);
      })
      .catch(function (err) {
        status("quote_invoice failed: " + errText(err));
      });
  }

  /* -------------------------------------------------------------- print & lock */

  /**
   * Locks or unlocks the whole card. A saved invoice is a printed document: nothing on
   * screen may still look editable, or an operator will "correct" a row that is already
   * in the books and on a customer's copy.
   */
  function setLocked(on) {
    locked = on;
    document.querySelector(".card--txn").classList.toggle("is-locked", on);

    $("mobile-input").disabled = on;
    $("search-btn").disabled = on;
    $("add-item-btn").disabled = on;
    $("payment-type").disabled = on;
    $("print-lock").hidden = on;
    $("print-preview").hidden = !on;
    $("new-txn").hidden = !on;

    Array.prototype.forEach.call(document.querySelectorAll(".qty-input"), function (input) {
      input.disabled = on;
    });
    Array.prototype.forEach.call(document.querySelectorAll(".row-del"), function (button) {
      button.disabled = on;
    });

    if (on) {
      hideNewCustomer();
      closePicker();
      $("new-txn").focus();
    }
  }

  /** Assembles what core needs to store this transaction. */
  function buildPayload() {
    return {
      customer_id: customer.id,
      date: null, // core stamps today
      payment_type: $("payment-type").value,
      lines: rows.map(function (row) {
        return { item_id: row.item_id, qty: row.qty, rate: row.rate, tax_rate: row.tax_rate };
      }),
    };
  }

  function saveError(message) {
    var msg = $("action-msg");
    msg.className = "action-msg error";
    msg.textContent = message;
    status("Not saved: " + message);
  }

  /**
   * Saves the transaction. The invoice number is allocated by core inside the same
   * transaction that writes the rows — never previewed here beforehand, so two consoles
   * billing at the same moment cannot be shown the same number.
   */
  function printAndLock() {
    if (locked) return; // already saved; New Transaction is the only way on

    if (!customer) return saveError("Attach a customer first.");
    if (!rows.length) return saveError("Add at least one item.");
    if (!lastQuote) return saveError("Totals not priced yet — try again.");

    var bad = rows.filter(function (row) {
      return !(row.qty > 0);
    });
    if (bad.length) return saveError("Every row needs a quantity above zero.");

    if (!invoke) return bridgeMissing("create_invoice");

    var button = $("print-lock");
    button.disabled = true;
    var msg = $("action-msg");
    msg.className = "action-msg muted";
    msg.textContent = "Saving…";

    invoke("create_invoice", {
      payload: buildPayload(),
      // What the panel is showing. Core refuses the save if it prices differently.
      expected: {
        subtotal: lastQuote.subtotal,
        cgst: lastQuote.cgst,
        sgst: lastQuote.sgst,
        igst: lastQuote.igst,
        grand_total: lastQuote.grand_total,
      },
    })
      .then(function (saved) {
        button.disabled = false;
        showSaved(saved);
      })
      .catch(function (err) {
        // Nothing was written — core rolls the whole transaction back — so the form stays
        // exactly as it was and the operator can retry.
        button.disabled = false;
        saveError(errText(err));
      });
  }

  /** Shows the allocated number, renders the stored figures, and locks the card. */
  function showSaved(saved) {
    var invoice = saved.invoice;

    var badge = $("inv-no");
    badge.textContent = invoice.invoice_no;
    badge.hidden = false;

    // Render what was actually stored, not what was on screen a moment ago.
    saved.lines.forEach(function (line, index) {
      var cell = document.querySelector('#items-body [data-total="' + index + '"]');
      if (cell) cell.textContent = money(line.line_total);
      if (rows[index]) rows[index].total = line.line_total;
    });
    renderTotals($("billing-totals"), totalsOf(invoice));

    // Keep the saved invoice for the print preview the locked card now offers.
    lastSaved = saved;
    setLocked(true);

    var msg = $("action-msg");
    msg.className = "action-msg ok";
    msg.textContent = "Saved — Invoice #" + invoice.invoice_no;
    status(
      "Saved " + invoice.invoice_no + " · " + rupees(invoice.grand_total) + " · " +
        saved.lines.length + " line(s) · " + saved.queued_sync_rows + " row(s) queued for sync."
    );
  }

  /** Clears the card back to a fresh empty state for the next customer. */
  function newTransaction() {
    customer = null;
    rows = [];
    lastQuote = null;
    lastSaved = null;

    $("mobile-input").value = "";
    $("payment-type").value = "cash";
    $("inv-no").hidden = true;
    $("inv-no").textContent = "";

    setLocked(false);

    var msg = $("action-msg");
    msg.className = "action-msg";
    msg.textContent = "";

    renderCustomer();
    renderRows();
    requote();
    $("mobile-input").focus();
    status("Ready for the next customer.");
  }

  /* --------------------------------------------------------------------- history */

  /** The invoice open in the detail view, or null while the list is showing. */
  var openDetail = null;

  function iso(date) {
    var pad = function (n) {
      return (n < 10 ? "0" : "") + n;
    };
    return date.getFullYear() + "-" + pad(date.getMonth() + 1) + "-" + pad(date.getDate());
  }

  /**
   * Turns the range control into `from`/`to` bounds. Weeks start Monday — the working
   * week a shop reconciles against, not the calendar's Sunday.
   */
  function rangeBounds() {
    var choice = $("f-range").value;
    var now = new Date();

    if (choice === "all") return { from: null, to: null };
    if (choice === "custom") {
      return { from: $("f-from").value || null, to: $("f-to").value || null };
    }
    if (choice === "today") return { from: iso(now), to: iso(now) };

    if (choice === "week") {
      var monday = new Date(now);
      var weekday = (now.getDay() + 6) % 7; // Monday = 0
      monday.setDate(now.getDate() - weekday);
      return { from: iso(monday), to: iso(now) };
    }

    var first = new Date(now.getFullYear(), now.getMonth(), 1);
    return { from: iso(first), to: iso(now) };
  }

  function loadHistory() {
    var body = $("history-body");
    var bounds = rangeBounds();
    var text = $("f-text").value.trim();

    if (!invoke) return bridgeMissing("list_invoices");

    status("Loading invoices…");
    invoke("list_invoices", {
      filter: { from: bounds.from, to: bounds.to, text: text || null, limit: null },
    })
      .then(function (list) {
        historyRows = list;
        $("history-empty").hidden = list.length > 0;

        body.innerHTML = list
          .map(function (row, index) {
            return (
              '<tr class="hist-row" data-open="' + index + '" tabindex="0" role="button">' +
              '<td class="mono hist-no">' + escapeHtml(row.invoice.invoice_no) + "</td>" +
              '<td class="mono">' + escapeHtml(stamp(row.invoice)) + "</td>" +
              "<td>" + escapeHtml(row.customer_name) + "</td>" +
              '<td class="mono">' + escapeHtml(row.invoice.payment_type) + "</td>" +
              '<td class="num mono">' + money(row.invoice.grand_total) + "</td>" +
              "</tr>"
            );
          })
          .join("");

        var sum = list.reduce(function (acc, row) {
          return acc + row.invoice.grand_total;
        }, 0);

        // A half-filled date box reads as empty, which silently drops that bound. Say so
        // rather than letting the list look narrower than it is.
        var open = [];
        if ($("f-range").value === "custom") {
          if (!bounds.from) open.push("no From date");
          if (!bounds.to) open.push("no To date");
        }
        var caveat = open.length ? " · open-ended (" + open.join(", ") + ")" : "";

        $("history-count").textContent = list.length
          ? list.length + " invoice(s) · " + rupees(sum) + " billed" + caveat
          : "";
        status(list.length + " invoice(s) in range.");
      })
      .catch(function (err) {
        body.innerHTML = "";
        $("history-empty").hidden = false;
        $("history-empty").textContent = "Could not load invoices: " + errText(err);
        status("list_invoices failed: " + errText(err));
      });
  }

  /** Opens the read-only detail. Nothing here can change a stored invoice. */
  function openInvoice(invoiceId) {
    if (!invoke) return bridgeMissing("invoice_detail");

    invoke("invoice_detail", { invoiceId: invoiceId })
      .then(function (detail) {
        if (!detail) {
          status("Invoice " + invoiceId + " not found.");
          return;
        }
        openDetail = detail;

        var invoice = detail.invoice;
        var buyer = detail.customer;

        $("d-inv-no").textContent = invoice.invoice_no;
        $("d-customer").innerHTML =
          '<span class="cust-name">' + escapeHtml(buyer.name) + "</span>" +
          ' <span class="cust-gstin">— GSTIN: ' +
          escapeHtml(buyer.gstin || "unregistered") + "</span>" +
          ' <span class="cust-pos">· ' + escapeHtml(buyer.mobile) +
          " · place of supply " + escapeHtml(buyer.place_of_supply) + "</span>";

        $("d-meta").innerHTML =
          '<span>Raised <strong class="mono">' + escapeHtml(stamp(invoice)) + "</strong></span>" +
          '<span>Payment <strong class="mono">' + escapeHtml(invoice.payment_type) +
          "</strong></span>" +
          '<span>Sync <strong class="mono">' + escapeHtml(invoice.sync_status) +
          "</strong></span>";

        // Same row renderer as the billing table, without the editable controls.
        $("d-lines").innerHTML = lineRowsHtml(detailLines(detail), false);
        renderTotals($("detail-totals"), totalsOf(invoice));

        $("d-gst-mode").textContent =
          (invoice.igst > 0 ? "Inter-state" : "Intra-state") +
          " · place of supply " + buyer.place_of_supply;

        $("history-list-card").hidden = true;
        $("history-detail").hidden = false;
        $("d-back").focus();
        status("Viewing " + invoice.invoice_no + " (read-only).");
      })
      .catch(function (err) {
        status("invoice_detail failed: " + errText(err));
      });
  }

  /** Saved lines in the shape the shared row renderer expects. */
  function detailLines(detail) {
    return detail.lines.map(function (entry) {
      return {
        item_code: entry.item_code,
        description: entry.description,
        uom: entry.uom,
        qty: entry.line.qty,
        rate: entry.line.rate,
        tax_rate: entry.line.tax_rate,
        total: entry.line.line_total,
      };
    });
  }

  function closeDetail() {
    openDetail = null;
    $("history-detail").hidden = true;
    $("history-list-card").hidden = false;
    status("Invoice history.");
  }

  $("f-range").addEventListener("change", function () {
    var custom = $("f-range").value === "custom";
    $("f-from-wrap").hidden = !custom;
    $("f-to-wrap").hidden = !custom;
    if (!custom) loadHistory();
  });

  $("f-apply").addEventListener("click", loadHistory);
  $("f-text").addEventListener("keydown", function (event) {
    if (event.key === "Enter") loadHistory();
  });
  $("f-from").addEventListener("change", loadHistory);
  $("f-to").addEventListener("change", loadHistory);

  $("history-body").addEventListener("click", function (event) {
    var tr = event.target.closest("[data-open]");
    if (tr) openInvoice(historyRows[Number(tr.dataset.open)].invoice.id);
  });
  $("history-body").addEventListener("keydown", function (event) {
    if (event.key !== "Enter" && event.key !== " ") return;
    var tr = event.target.closest("[data-open]");
    if (tr) {
      event.preventDefault();
      openInvoice(historyRows[Number(tr.dataset.open)].invoice.id);
    }
  });

  $("d-back").addEventListener("click", closeDetail);

  /* -------------------------------------------------------------- printable sheet */

  /**
   * The printable document, built from a saved invoice. Print & Lock and Reprint both
   * come through here, so a reprint is the same layout and the same numbers — nothing is
   * re-entered or recomputed to produce it.
   */
  function renderPrintable(detail, label) {
    var invoice = detail.invoice;
    var buyer = detail.customer;
    var lines = detailLines(detail);

    var taxRows = invoice.igst > 0
      ? "<tr><th>IGST</th><td>" + rupees(invoice.igst) + "</td></tr>"
      : "<tr><th>CGST</th><td>" + rupees(invoice.cgst) + "</td></tr>" +
        "<tr><th>SGST</th><td>" + rupees(invoice.sgst) + "</td></tr>";

    $("print-sheet").innerHTML =
      '<div class="sheet-head">' +
        "<div><h1>TAX INVOICE</h1>" +
        '<p class="sheet-seller">RealInvoice Demo Traders · Tamil Nadu · Node POS-01</p></div>' +
        '<div class="sheet-no"><strong>' + escapeHtml(invoice.invoice_no) + "</strong>" +
        "<span>" + escapeHtml(stamp(invoice)) + "</span></div>" +
      "</div>" +
      '<div class="sheet-buyer"><strong>Billed to</strong>' +
        "<div>" + escapeHtml(buyer.name) + "</div>" +
        "<div>GSTIN: " + escapeHtml(buyer.gstin || "unregistered") + "</div>" +
        "<div>" + escapeHtml(buyer.mobile) + " · Place of supply " +
        escapeHtml(buyer.place_of_supply) + "</div></div>" +
      '<table class="sheet-lines"><thead><tr>' +
        "<th>#</th><th>Item</th><th>Description</th><th>UOM</th>" +
        '<th class="num">Qty</th><th class="num">Rate</th>' +
        '<th class="num">Tax %</th><th class="num">Amount</th>' +
      "</tr></thead><tbody>" +
      lines
        .map(function (line, index) {
          return (
            "<tr><td>" + (index + 1) + "</td>" +
            "<td>" + escapeHtml(line.item_code) + "</td>" +
            "<td>" + escapeHtml(line.description) + "</td>" +
            "<td>" + escapeHtml(line.uom) + "</td>" +
            '<td class="num">' + line.qty + "</td>" +
            '<td class="num">' + money(line.rate) + "</td>" +
            '<td class="num">' + money(line.tax_rate) + "</td>" +
            '<td class="num">' + money(line.total) + "</td></tr>"
          );
        })
        .join("") +
      "</tbody></table>" +
      '<table class="sheet-totals">' +
        "<tr><th>Subtotal</th><td>" + rupees(invoice.subtotal) + "</td></tr>" +
        taxRows +
        '<tr class="grand"><th>Grand Total</th><td>' + rupees(invoice.grand_total) +
        "</td></tr>" +
      "</table>" +
      '<p class="sheet-foot">Payment: ' + escapeHtml(invoice.payment_type) +
        " · This is a computer-generated invoice.</p>";

    $("print-label").textContent = label;
    $("print-overlay").hidden = false;
    $("print-close").focus();
  }

  $("print-close").addEventListener("click", function () {
    $("print-overlay").hidden = true;
  });

  $("print-now").addEventListener("click", function () {
    // The real printer call is the browser's own dialog; no printer configuration or
    // driver selection is wired up yet.
    try {
      window.print();
    } catch (err) {
      status("Printing unavailable here: " + errText(err));
    }
  });

  // Reprint: the same layout, from the saved invoice already on screen.
  $("d-reprint").addEventListener("click", function () {
    if (openDetail) renderPrintable(openDetail, "Reprint · " + openDetail.invoice.invoice_no);
  });

  // Print & Lock's own preview, from what the save returned.
  $("print-preview").addEventListener("click", function () {
    if (!lastSaved) return;
    renderPrintable(
      { invoice: lastSaved.invoice, customer: lastSaved.customer, lines: savedToDetailLines(lastSaved) },
      lastSaved.invoice.invoice_no
    );
  });

  /** A just-saved invoice reshaped to match what `invoice_detail` returns. */
  function savedToDetailLines(saved) {
    return saved.lines.map(function (line, index) {
      var source = rows[index] || {};
      return {
        line: line,
        item_code: source.item_code || "",
        description: source.description || "",
        uom: source.uom || "",
      };
    });
  }

  document.addEventListener("keydown", function (event) {
    if (event.key !== "Escape") return;
    if (!$("print-overlay").hidden) $("print-overlay").hidden = true;
    else if (!$("history-detail").hidden) closeDetail();
  });

  /* ----------------------------------------------------------------- wiring */

  $("search-btn").addEventListener("click", searchCustomer);
  $("mobile-input").addEventListener("keydown", function (event) {
    if (event.key === "Enter") searchCustomer();
  });

  $("nc-save").addEventListener("click", createCustomer);
  $("nc-cancel").addEventListener("click", function () {
    hideNewCustomer();
    status("New customer cancelled.");
  });

  $("print-lock").addEventListener("click", printAndLock);
  $("new-txn").addEventListener("click", newTransaction);
  document.addEventListener("keydown", function (event) {
    if (event.key === "F5") {
      event.preventDefault(); // never let F5 reload the shell mid-transaction
      printAndLock();
    }
  });

  loadNodeStatus();
  showPane("billing");
  renderCustomer();
  renderRows();
  requote();
  $("mobile-input").focus();
})();
