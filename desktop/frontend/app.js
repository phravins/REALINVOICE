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
    function row(label, value, grand) {
      return (
        '<div class="flex items-baseline justify-between gap-4 ' +
        (grand
          ? 'mt-2 border-t border-base-300 pt-3"><dt class="text-base font-semibold">'
          : 'py-1"><dt class="text-sm text-base-content/60">') +
        label +
        "</dt><dd class=" +
        (grand
          ? '"font-mono text-2xl font-semibold tabular-nums">'
          : '"font-mono text-sm tabular-nums">') +
        rupees(value) +
        "</dd></div>"
      );
    }

    var html = row("Subtotal", t.subtotal);
    if (t.intra_state) {
      html += row("CGST", t.cgst) + row("SGST", t.sgst);
    } else {
      html += row("IGST", t.igst);
    }
    el.innerHTML = html + row("Grand Total", t.grand_total, true);
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
          ? '<input class="h-7 w-full max-w-20 rounded-field border border-base-300 ' +
            'bg-base-100 px-2 text-right font-mono text-sm tabular-nums ' +
            'focus:border-base-content/30 focus:outline-none focus:ring-2 ' +
            'focus:ring-base-content/10" type="number" min="0" step="any" value="' +
            line.qty + '" data-index="' + index + '" aria-label="Quantity for ' +
            escapeHtml(line.item_code) + '" />'
          : '<span class="font-mono tabular-nums">' + line.qty + "</span>";

        var remove = editable
          ? '<td class="text-right"><button class="flex size-7 items-center ' +
            'justify-center rounded-field text-base-content/45 transition-colors ' +
            'hover:bg-base-200 hover:text-error" data-remove="' + index +
            '" title="Remove row" aria-label="Remove ' + escapeHtml(line.item_code) +
            '"><span class="hero-x-mark size-4" aria-hidden="true"></span></button></td>'
          : "";

        var num = ' class="py-2 text-right font-mono tabular-nums"';

        return (
          '<tr class="border-b border-base-300 text-sm last:border-0 hover:bg-base-200/60">' +
          '<td class="py-2 font-mono">' + escapeHtml(line.item_code) + "</td>" +
          '<td class="py-2">' + escapeHtml(line.description) + "</td>" +
          '<td class="py-2 text-right">' + qty + "</td>" +
          "<td" + num + ">" + money(line.rate) + "</td>" +
          "<td" + num + ">" + money(line.tax_rate) + "</td>" +
          '<td class="py-2 text-right font-mono tabular-nums" data-total="' + index +
          '">' + money(line.total) + "</td>" +
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

  var navItems = Array.prototype.slice.call(document.querySelectorAll(".nav-item"));

  function showPane(name) {
    navItems.forEach(function (item) {
      var active = item.dataset.pane === name;
      item.classList.toggle("is-active", active);
      item.setAttribute("aria-selected", active ? "true" : "false");
    });
    Array.prototype.forEach.call(document.querySelectorAll(".pane"), function (pane) {
      pane.classList.toggle("is-active", pane.id === "pane-" + name);
    });
    if (name === "history") {
      // Reload on every visit so an invoice saved a moment ago is already listed.
      loadHistory();
    } else if (name === "settings") {
      refreshAbout();
      status("About RealInvoice.");
    } else if (name === "inventory") {
      loadCatalogue();
      status("The catalogue the billing screen prices from.");
    } else if (name === "analytics") {
      loadAnalytics();
    } else if (name === "sync") {
      refreshSync();
      status("Copies of what this till bills are sent to the back office.");
    } else if (name === "users") {
      showUserForm(false);
      loadUsers();
      status("Accounts that can sign in on this machine.");
    } else if (name !== "billing") {
      status(name + ": coming soon.");
    }
  }

  navItems.forEach(function (item) {
    item.addEventListener("click", function () {
      showPane(item.dataset.pane);
    });
  });

  /* --------------------------------------------------------- title bar status */

  /** The status bar's line. The badge is painted by refreshSync() from the worker. */
  function loadNodeStatus() {
    if (!invoke) return bridgeMissing("node_status");
    invoke("node_status")
      .then(function (info) {
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
      line.innerHTML = "<span>Search a mobile number to attach a customer.</span>";
      return;
    }
    line.innerHTML =
      '<span class="font-medium text-base-content">' + escapeHtml(customer.name) + "</span>" +
      ' <span class="text-base-content/60">— GSTIN: ' +
      escapeHtml(customer.gstin || "unregistered") +
      "</span>" +
      ' <span class="text-base-content/45">· ' + escapeHtml(customer.mobile) +
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
    // Hooked on the data attribute, not on a styling class: what makes this input the
    // quantity is that it carries a row index, and that survives a restyle.
    var input = event.target.closest("input[data-index]");
    if (!input) return;

    var qty = parseFloat(input.value);
    var valid = isFinite(qty) && qty > 0;
    input.classList.toggle("border-error", !valid && input.value !== "");
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
      list.innerHTML =
        '<li class="px-3 py-2 text-sm text-error">Tauri bridge unavailable.</li>';
      return;
    }

    invoke("search_item", { query: query })
      .then(function (items) {
        if (!items.length) {
          list.innerHTML =
            '<li class="px-3 py-2 text-sm text-base-content/60">No items match.</li>';
          return;
        }
        list.innerHTML = items
          .map(function (item, index) {
            return (
              '<li><button type="button" data-pick="' + index + '" ' +
              'class="flex w-full items-baseline gap-3 rounded-field px-3 py-2 ' +
              'text-left text-sm transition-colors hover:bg-base-200">' +
              '<span class="w-32 shrink-0 font-mono">' + escapeHtml(item.item_code) +
              "</span>" +
              '<span class="min-w-0 flex-1 truncate">' + escapeHtml(item.description) +
              "</span>" +
              '<span class="shrink-0 font-mono text-xs text-base-content/45">₹' +
              money(item.rate) + " · " + money(item.tax_rate) + "% · " +
              escapeHtml(item.uom) + "</span>" +
              "</button></li>"
            );
          })
          .join("");
        list.dataset.items = JSON.stringify(items);
      })
      .catch(function (err) {
        list.innerHTML =
          '<li class="px-3 py-2 text-sm text-error">' + escapeHtml(errText(err)) + "</li>";
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
    if (event.key === "Escape" && !$("item-picker").hidden) {
      closePicker();
      event.stopImmediatePropagation();
    }
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
   * error wrong, and a stale line next to fresh totals is how a counter mis-bills.
   * The saved confirmation is not kept here — the banner carries it.
   */
  function clearMessage() {
    var msg = $("action-msg");
    msg.className = "action-msg";
    msg.textContent = "";
  }

  /** Re-prices every row through core. Called on any change to rows or customer. */
  function requote() {
    clearMessage();
    if (!invoke) return bridgeMissing("quote_invoice");
    // Every command is gated on a session; there is nothing to price before sign-in.
    if (!user) return;

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

    // The editing header (customer search) and the editing controls are removed, not
    // disabled. A row of greyed-out buttons reads as broken; their absence reads as
    // finished.
    $("txn-head").hidden = on;
    $("saved-banner").hidden = !on;
    $("add-item").hidden = on;
    $("txn-actions").hidden = on;

    // "Saving…" was the last thing this row said; the banner has superseded it.
    clearMessage();

    // Payment becomes a stated fact rather than a choice still on offer.
    $("pay-group").hidden = on;
    $("pay-static").hidden = !on;
    $("upi-box").hidden = on || selectedPayment() !== "upi";

    // Re-rendering swaps the Qty boxes and remove buttons for plain text, because
    // lineRowsHtml only draws them when the table is editable.
    renderRows();

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
      payment_type: selectedPayment(),
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

    $("saved-title").textContent = "Invoice " + invoice.invoice_no + " saved";
    $("saved-sub").textContent =
      rupees(invoice.grand_total) + " · " + saved.lines.length + " item" +
      (saved.lines.length === 1 ? "" : "s") + " · queued for sync";
    $("pay-static").textContent = invoice.payment_type.toUpperCase();

    // Keep the saved invoice for the print preview the locked card now offers.
    lastSaved = saved;
    setLocked(true);

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
    setPayment("cash");
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
              '<tr class="cursor-pointer border-b border-base-300 text-sm last:border-0 ' +
              'hover:bg-base-200/60 focus:bg-base-200/60 focus:outline-none" data-open="' +
              index + '" tabindex="0" role="button">' +
              '<td class="py-2 font-mono font-medium text-base-content">' +
              escapeHtml(row.invoice.invoice_no) + "</td>" +
              '<td class="py-2 font-mono text-base-content/60">' +
              escapeHtml(stamp(row.invoice)) + "</td>" +
              '<td class="py-2">' + escapeHtml(row.customer_name) + "</td>" +
              '<td class="py-2 font-mono text-base-content/60">' +
              escapeHtml(row.invoice.payment_type) + "</td>" +
              '<td class="py-2 text-right font-mono tabular-nums">' +
              money(row.invoice.grand_total) + "</td>" +
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
        // Addressed by position, not by a styling class: the two <p>s are the title and
        // the hint, and that stays true however they are styled.
        var lines = $("history-empty").querySelectorAll("p");
        lines[0].textContent = "Could not load invoices";
        lines[1].textContent = errText(err);
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
          '<span class="font-medium text-base-content">' + escapeHtml(buyer.name) + "</span>" +
          ' <span class="text-base-content/60">— GSTIN: ' +
          escapeHtml(buyer.gstin || "unregistered") + "</span>" +
          ' <span class="text-base-content/45">· ' + escapeHtml(buyer.mobile) +
          " · place of supply " + escapeHtml(buyer.place_of_supply) + "</span>";

        function meta(label, value) {
          return (
            "<span>" + label + ' <strong class="font-medium text-base-content">' +
            escapeHtml(value) + "</strong></span>"
          );
        }

        $("d-meta").innerHTML =
          meta("Raised", stamp(invoice)) +
          meta("Payment", invoice.payment_type) +
          meta("Sync", invoice.sync_status);

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
    if (!$("user-menu").hidden) {
      closeUserMenu();
      event.stopImmediatePropagation();
    } else if (!$("print-overlay").hidden) {
      $("print-overlay").hidden = true;
      event.stopImmediatePropagation();
    } else if (!$("history-detail").hidden) {
      closeDetail();
      event.stopImmediatePropagation();
    }
  });

  /* --------------------------------------------------------------------- sign-in */

  /** The signed-in user, or null. Mirrors the session the backend holds. */
  var user = null;

  /** `isError` separates a failure from a plain notice like "Signed out." */
  function showLogin(message, isError) {
    user = null;
    $("shell").hidden = true;
    $("setup-screen").hidden = true;
    $("login-screen").hidden = false;
    $("login-pass").value = "";
    setMsg("login-msg", message, isError);
    $("login-user").focus();
  }

  /** The first-run screen, shown in place of the gate when there are no accounts yet. */
  function showSetup() {
    user = null;
    $("shell").hidden = true;
    $("login-screen").hidden = true;
    $("setup-screen").hidden = false;
    $("setup-name").focus();
  }

  function showShell() {
    $("login-screen").hidden = true;
    $("setup-screen").hidden = true;
    $("shell").hidden = false;
    // Cashiers never see the Users item. The commands behind it refuse them anyway.
    $("nav-users").hidden = user.role !== "owner";
    $("avatar-initial").textContent = (user.display_name || user.username || "?").trim().charAt(0);
    $("menu-name").textContent = user.display_name;
    $("menu-role").textContent = user.role + " · " + user.username;
    loadNodeStatus();
    refreshAbout();
    startSyncPolling();
    showPane("billing");
    $("mobile-input").focus();
  }

  function signIn(event) {
    if (event) event.preventDefault();
    if (!invoke) return bridgeMissing("login");

    var username = $("login-user").value.trim();
    var password = $("login-pass").value;
    if (!username || !password) {
      setMsg("login-msg", "Enter a username and password.", true);
      return;
    }

    var button = $("login-submit");
    button.disabled = true;
    setMsg("login-msg", "Signing in…", false);

    invoke("login", { username: username, password: password })
      .then(function (session) {
        button.disabled = false;
        user = session.user;
        setMsg("login-msg", "", false);
        newTransaction();
        showShell();
        status("Signed in as " + user.display_name + ".");
      })
      .catch(function (err) {
        button.disabled = false;
        $("login-pass").value = "";
        setMsg("login-msg", errText(err), true);
        $("login-pass").focus();
      });
  }

  function signOut() {
    if (!invoke) return bridgeMissing("logout");
    stopSyncPolling();
    invoke("logout").then(function () {
      // Clear the screen before showing the gate, so a half-billed transaction is not
      // sitting there for whoever signs in next.
      newTransaction();
      historyRows = [];
      $("history-body").innerHTML = "";
      closeDetail();
      showLogin("Signed out.");
    });
  }

  /** Asks the backend who is signed in, and shows the gate or the shell accordingly. */
  function bootstrap() {
    if (!invoke) {
      showLogin("Tauri bridge unavailable — run this through the desktop app.", true);
      return;
    }

    invoke("auth_status")
      .then(function (info) {
        $("login-node").textContent = "Office Console · Node " + info.node;

        if (info.session) {
          user = info.session.user;
          showShell();
        } else if (info.needs_setup) {
          showSetup();
        } else {
          showLogin("");
        }
      })
      .catch(function (err) {
        showLogin(errText(err), true);
      });
  }

  $("login-form").addEventListener("submit", signIn);

  /* ------------------------------------------------------------- first-run setup */

  /**
   * Shared by both places an account is created. Returns an error string, or "" when the
   * details are usable. The same rules are enforced in core — this is here so somebody
   * mistyping a password finds out before a round trip, not instead of one.
   */
  function accountProblem(displayName, username, password, confirm) {
    if (!displayName) return "Enter a display name.";
    if (!username) return "Enter a username.";
    if (/\s/.test(username)) return "A username cannot contain spaces.";
    if (password.length < 8) return "The password must be at least 8 characters.";
    if (password !== confirm) return "The two passwords do not match.";
    return "";
  }

  function setMsg(id, text, isError) {
    var el = $(id);
    var base =
      id === "setup-msg" || id === "login-msg"
        ? "min-h-[18px] text-center text-xs "
        : "text-xs ";
    el.className = base + (isError ? "text-error" : "text-base-content/60");
    el.textContent = text || "";
  }

  function createFirstUser(event) {
    if (event) event.preventDefault();
    if (!invoke) return bridgeMissing("create_first_user");

    var displayName = $("setup-name").value.trim();
    var username = $("setup-user").value.trim();
    var password = $("setup-pass").value;
    var problem = accountProblem(displayName, username, password, $("setup-pass2").value);
    if (problem) return setMsg("setup-msg", problem, true);

    var button = $("setup-submit");
    button.disabled = true;
    setMsg("setup-msg", "Creating your account…", false);

    invoke("create_first_user", {
      displayName: displayName,
      username: username,
      password: password,
    })
      .then(function (session) {
        button.disabled = false;
        // Straight into the shell: they just chose these credentials, so asking them to
        // type them again would be ceremony, not security.
        user = session.user;
        $("setup-pass").value = "";
        $("setup-pass2").value = "";
        setMsg("setup-msg", "", false);
        newTransaction();
        showShell();
        status("Welcome, " + user.display_name + ". This account owns this machine.");
      })
      .catch(function (err) {
        button.disabled = false;
        setMsg("setup-msg", errText(err), true);
      });
  }

  $("setup-form").addEventListener("submit", createFirstUser);

  /* -------------------------------------------------------------------- users */

  /** The account list, in the same label/value rows the About page uses. */
  function loadUsers() {
    if (!invoke || !user || user.role !== "owner") return;

    invoke("list_users")
      .then(function (users) {
        // One block of rows per account, separated by space rather than by a card. The
        // signed-in account is marked in place; there is no heading repeating the name
        // that the first row already gives.
        $("user-list").innerHTML = users
          .map(function (u, index) {
            function pair(label, valueHtml, first) {
              var edge = first ? "" : " border-t border-base-300";
              return (
                '<dt class="py-3 text-sm text-base-content/60' + edge + '">' + label +
                '</dt><dd class="py-3 text-sm text-base-content' + edge + '">' +
                valueHtml + "</dd>"
              );
            }

            var you = u.id === user.id
              ? ' <span class="ml-2 rounded-field border border-base-300 px-2 py-0.5 ' +
                'align-middle text-2xs font-normal text-base-content/60">you</span>'
              : "";

            // Accounts are separated by space and a rule, not by a panel.
            return (
              '<dl class="grid grid-cols-[minmax(140px,220px)_1fr]' +
              (index === 0 ? "" : " mt-6 border-t border-base-300 pt-6") + '">' +
              pair("Display Name", escapeHtml(u.display_name) + you, true) +
              pair("Username", escapeHtml(u.username)) +
              pair("Role", escapeHtml(u.role === "owner" ? "Owner" : "Cashier")) +
              pair("Created", escapeHtml(dateOnly(u.created_at))) +
              "</dl>"
            );
          })
          .join("");
      })
      .catch(function (err) {
        $("user-list").innerHTML =
          '<p class="py-6 text-center text-sm text-error">' + escapeHtml(errText(err)) +
          "</p>";
      });
  }

  /** "2025-04-07T18:22:10" → "07 Apr 2025". Timestamps are stored local, so no parsing. */
  function dateOnly(stamp) {
    var months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    var parts = String(stamp || "").slice(0, 10).split("-");
    if (parts.length !== 3) return stamp || "—";
    return parts[2] + " " + (months[Number(parts[1]) - 1] || parts[1]) + " " + parts[0];
  }

  function showUserForm(on) {
    $("user-form").hidden = !on;
    $("user-add-toggle").hidden = on;
    if (on) $("user-name").focus();
  }

  function resetUserForm() {
    $("user-name").value = "";
    $("user-user").value = "";
    $("user-pass").value = "";
    $("user-pass2").value = "";
    $("user-role").value = "cashier";
    setMsg("user-msg", "", false);
  }

  function createUser(event) {
    if (event) event.preventDefault();
    if (!invoke) return bridgeMissing("create_user");

    var displayName = $("user-name").value.trim();
    var username = $("user-user").value.trim();
    var password = $("user-pass").value;
    var problem = accountProblem(displayName, username, password, $("user-pass2").value);
    if (problem) return setMsg("user-msg", problem, true);

    var button = $("user-save");
    button.disabled = true;
    setMsg("user-msg", "Creating…", false);

    invoke("create_user", {
      displayName: displayName,
      username: username,
      password: password,
      role: $("user-role").value,
    })
      .then(function (created) {
        button.disabled = false;
        resetUserForm();
        showUserForm(false);
        loadUsers();
        status(created.display_name + " can now sign in as a " + created.role + ".");
      })
      .catch(function (err) {
        button.disabled = false;
        setMsg("user-msg", errText(err), true);
      });
  }

  $("user-add-toggle").addEventListener("click", function () {
    resetUserForm();
    showUserForm(true);
  });
  $("user-cancel").addEventListener("click", function () {
    resetUserForm();
    showUserForm(false);
  });
  $("user-form").addEventListener("submit", createUser);
  /* ------------------------------------------------------------------ user menu */

  function closeUserMenu() {
    $("user-menu").hidden = true;
    $("avatar-btn").setAttribute("aria-expanded", "false");
  }

  function toggleUserMenu() {
    var open = $("user-menu").hidden;
    $("user-menu").hidden = !open;
    $("avatar-btn").setAttribute("aria-expanded", open ? "true" : "false");
  }

  $("avatar-btn").addEventListener("click", function (event) {
    event.stopPropagation();
    toggleUserMenu();
  });

  $("menu-about").addEventListener("click", function () {
    closeUserMenu();
    showPane("settings");
  });

  $("menu-signout").addEventListener("click", function () {
    closeUserMenu();
    signOut();
  });

  // Any click outside the menu dismisses it.
  document.addEventListener("click", function (event) {
    if (!$("user-menu").hidden && !event.target.closest(".avatar-wrap")) closeUserMenu();
  });

  /* -------------------------------------------------------------- payment type */

  function selectedPayment() {
    var chosen = document.querySelector('input[name="payment"]:checked');
    return chosen ? chosen.value : "cash";
  }

  function setPayment(value) {
    Array.prototype.forEach.call(document.querySelectorAll('input[name="payment"]'), function (radio) {
      radio.checked = radio.value === value;
    });
    $("upi-box").hidden = value !== "upi";
  }

  Array.prototype.forEach.call(document.querySelectorAll('input[name="payment"]'), function (radio) {
    radio.addEventListener("change", function () {
      // UPI is the only one that needs anything on screen; the box is a placeholder
      // until QR generation lands.
      $("upi-box").hidden = radio.value !== "upi";
      status("Payment: " + radio.value.toUpperCase());
    });
  });

  /* ---------------------------------------------------------------- shortcuts */

  /**
   * Clears an in-progress transaction. Confirms first if anything has been billed — a
   * stray Esc must not wipe a half-entered invoice.
   */
  function clearTransaction() {
    if (locked) return; // a saved invoice is cleared with New Transaction, not Esc

    if (!rows.length && !customer) {
      status("Nothing to clear.");
      return;
    }
    if (rows.length && !window.confirm("Clear this transaction? " + rows.length + " item(s) will be discarded.")) {
      return;
    }
    newTransaction();
    status("Transaction cleared.");
  }

  document.addEventListener("keydown", function (event) {
    // The login screen takes no shortcuts at all, Esc included.
    if (!$("login-screen").hidden) return;

    if (event.key === "F2") {
      event.preventDefault();
      if (!locked) {
        showPane("billing");
        openPicker();
      }
      return;
    }

    if (event.key === "F9") {
      event.preventDefault();
      // Stub: the replicator is the sync worker, which does not exist yet.
      status("F9 · Replicator Settings — coming with the sync stage.");
      return;
    }

    if (event.key === "Escape") {
      // Overlays and the detail view close first; only a bare Esc clears the counter.
      if (!$("print-overlay").hidden || !$("history-detail").hidden) return;
      if (!$("item-picker").hidden) return;
      if ($("pane-billing").classList.contains("is-active")) clearTransaction();
    }
  });

  /* ----------------------------------------------------------------------- theme */

  /**
   * boot.js has already set data-theme from the localStorage cache or the OS, so the
   * first paint was correct. This reconciles with core's settings table, which is the
   * authority — the cache only exists to avoid a flash.
   */
  function applyTheme(theme) {
    document.documentElement.setAttribute("data-theme", theme);
    try {
      localStorage.setItem("ui.theme", theme);
    } catch (err) {
      // Blocked storage costs us the no-flash boot, nothing else.
    }
  }

  function currentTheme() {
    return document.documentElement.getAttribute("data-theme") === "light" ? "light" : "dark";
  }

  function loadTheme() {
    if (!invoke) return;
    invoke("get_theme")
      .then(function (saved) {
        // Nothing saved yet: keep whatever the OS gave us and leave it unsaved, so the
        // app keeps following the OS until someone actually chooses.
        if (saved === "light" || saved === "dark") applyTheme(saved);
      })
      .catch(function (err) {
        status("get_theme failed: " + errText(err));
      });
  }

  function toggleTheme() {
    var next = currentTheme() === "dark" ? "light" : "dark";
    applyTheme(next);
    refreshAbout();

    if (!invoke) return;
    invoke("set_theme", { theme: next }).catch(function (err) {
      status("Theme not saved: " + errText(err));
    });
  }

  $("theme-toggle").addEventListener("click", toggleTheme);

  /* ----------------------------------------------------------------------- about */

  /** Everything here is read from the running build; nothing is duplicated in the UI. */
  function refreshAbout() {
    if (!invoke || !user) return;

    invoke("app_info")
      .then(function (info) {
        // The sidebar's version line. Guarded: this pane must still render its rows if
        // the shell ever stops carrying that element.
        var sideApp = $("side-app");
        if (sideApp) sideApp.textContent = info.name + " " + info.version;

        rowList($("about-rows"), [
          ["Username", user.username],
          ["Display name", user.display_name],
          ["Role", user.role],
          ["Node", info.node],
          ["Theme", currentTheme() === "dark" ? "Dark" : "Light"],
        ]);

        rowList($("about-app-rows"), [
          ["Application", info.name],
          ["Version", info.version],
          ["Identifier", info.identifier],
          ["Build date", info.build_date],
          ["Database", info.db_path],
        ]);
      })
      .catch(function (err) {
        rowList($("about-rows"), [["Error", errText(err)]]);
      });
  }

  /** Label-left / value-right rows, separated by dividers — no boxes. */
  function rowList(el, rows) {
    if (!el) return;
    el.innerHTML = rows
      .map(function (row, index) {
        // The first row's divider would double up with the heading's spacing.
        var edge = index === 0 ? "" : " border-t border-base-300";
        return (
          '<dt class="py-3 text-sm text-base-content/60' + edge + '">' +
          escapeHtml(row[0]) +
          '</dt><dd class="py-3 text-sm text-base-content break-words' + edge + '">' +
          escapeHtml(row[1]) +
          "</dd>"
        );
      })
      .join("");
  }


  /* ------------------------------------------------------------------ inventory */

  /** The catalogue, as last loaded. Kept so the edit button can prefill from it. */
  var catalogue = [];

  function loadCatalogue() {
    if (!invoke || !user) return;

    var text = $("it-filter").value.trim();
    invoke("list_items", { filter: { text: text || null } })
      .then(function (items) {
        catalogue = items;
        $("catalogue-body").innerHTML = items
          .map(function (item, index) {
            return (
              '<tr class="border-b border-base-300 text-sm last:border-0 hover:bg-base-200/60">' +
              '<td class="py-2 font-mono">' + escapeHtml(item.item_code) + "</td>" +
              '<td class="py-2">' + escapeHtml(item.description) + "</td>" +
              '<td class="py-2 text-right font-mono tabular-nums">' + money(item.rate) + "</td>" +
              '<td class="py-2 pr-4 text-right font-mono tabular-nums">' +
              money(item.tax_rate) + "</td>" +
              '<td class="py-2 text-base-content/60">' + escapeHtml(item.uom) + "</td>" +
              '<td class="py-2 text-right"><button type="button" data-edit="' + index +
              '" title="Edit ' + escapeHtml(item.item_code) + '" aria-label="Edit ' +
              escapeHtml(item.item_code) +
              '" class="flex size-7 items-center justify-center rounded-field ' +
              'text-base-content/45 transition-colors hover:bg-base-200 ' +
              'hover:text-base-content"><span class="hero-pencil-square size-4" ' +
              'aria-hidden="true"></span></button></td>' +
              "</tr>"
            );
          })
          .join("");

        $("catalogue-empty").hidden = items.length > 0;
        return invoke("count_items");
      })
      .then(function (total) {
        var shown = catalogue.length;
        $("catalogue-count").textContent =
          shown === total
            ? total + " item(s)"
            : shown + " of " + total + " item(s) match";
      })
      .catch(function (err) {
        $("catalogue-body").innerHTML = "";
        $("catalogue-empty").hidden = false;
        status("list_items failed: " + errText(err));
      });
  }

  function showItemForm(on, existing) {
    $("item-form").hidden = !on;
    $("item-add-toggle").hidden = on;
    if (!on) return;

    $("item-form-head").textContent = existing ? "Edit item" : "Add item";
    $("it-code").value = existing ? existing.item_code : "";
    $("it-desc").value = existing ? existing.description : "";
    $("it-rate").value = existing ? existing.rate : "";
    $("it-tax").value = existing ? String(existing.tax_rate) : "18";
    $("it-uom").value = existing ? existing.uom : "";
    // The code is the key core matches on, so changing it while editing would quietly
    // create a second item rather than rename this one.
    $("it-code").readOnly = !!existing;
    $("it-code").classList.toggle("bg-base-200", !!existing);
    setMsg("it-msg", "", false);
    (existing ? $("it-desc") : $("it-code")).focus();
  }

  function saveItem(event) {
    if (event) event.preventDefault();
    if (!invoke) return bridgeMissing("save_item");

    var code = $("it-code").value.trim();
    var description = $("it-desc").value.trim();
    var rate = parseFloat($("it-rate").value);

    if (!code) return setMsg("it-msg", "Enter an item code.", true);
    if (!description) return setMsg("it-msg", "Enter a description.", true);
    if (!isFinite(rate) || rate < 0) return setMsg("it-msg", "Enter a rate of 0 or more.", true);

    var button = $("it-save");
    button.disabled = true;
    setMsg("it-msg", "Saving…", false);

    invoke("save_item", {
      item: {
        item_code: code,
        description: description,
        rate: rate,
        tax_rate: parseFloat($("it-tax").value),
        uom: $("it-uom").value.trim(),
      },
    })
      .then(function (saved) {
        button.disabled = false;
        showItemForm(false);
        loadCatalogue();
        status("Saved " + saved.item_code + ".");
      })
      .catch(function (err) {
        button.disabled = false;
        setMsg("it-msg", errText(err), true);
      });
  }

  $("item-add-toggle").addEventListener("click", function () {
    showItemForm(true, null);
  });
  $("it-cancel").addEventListener("click", function () {
    showItemForm(false);
  });
  $("item-form").addEventListener("submit", saveItem);
  $("it-filter").addEventListener("input", loadCatalogue);
  $("catalogue-body").addEventListener("click", function (event) {
    var button = event.target.closest("[data-edit]");
    if (button) showItemForm(true, catalogue[Number(button.dataset.edit)]);
  });

  /* ------------------------------------------------------------------ analytics */

  /** Turns the Analytics range control into the bounds core filters on. */
  function analyticsRange() {
    var choice = $("an-range").value;
    var now = new Date();

    if (choice === "all") return { from: null, to: null };

    if (choice === "week") {
      var monday = new Date(now);
      monday.setDate(now.getDate() - ((now.getDay() + 6) % 7));
      return { from: iso(monday), to: iso(now) };
    }

    if (choice === "month") {
      return { from: iso(new Date(now.getFullYear(), now.getMonth(), 1)), to: iso(now) };
    }

    // The Indian financial year: April to March. The same rule the invoice numbers use,
    // so "this financial year" here means the same span as the RI-YYYY- prefix.
    var startYear = now.getMonth() >= 3 ? now.getFullYear() : now.getFullYear() - 1;
    return { from: iso(new Date(startYear, 3, 1)), to: iso(now) };
  }

  /** `2026-04-01` -> `01 Apr`, for the chart's axis. */
  function dayLabel(date) {
    var months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    var parts = String(date || "").split("-");
    if (parts.length !== 3) return date || "";
    return parts[2] + " " + (months[Number(parts[1]) - 1] || parts[1]);
  }

  /** A round number at or above `value`, so the chart's gridlines read sensibly. */
  function niceCeiling(value) {
    if (!(value > 0)) return 100;
    var magnitude = Math.pow(10, Math.floor(Math.log10(value)));
    return Math.ceil(value / magnitude) * magnitude;
  }

  function loadAnalytics() {
    if (!invoke || !user) return;

    invoke("analytics", { range: analyticsRange() })
      .then(function (report) {
        var s = report.summary;
        var empty = s.invoice_count === 0;

        $("an-empty").hidden = !empty;
        ["an-stats", "an-chart", "an-payments", "an-top"].forEach(function (id) {
          var host = $(id).closest(".rounded-box") || $(id);
          host.hidden = empty;
        });
        if (empty) {
          status("Nothing billed in this range.");
          return;
        }

        $("an-stats").innerHTML =
          UI.statCard("hero-document-text", "Invoices", String(s.invoice_count)) +
          UI.statCard("hero-banknotes", "Billed", rupees(s.grand_total),
                      "including tax") +
          UI.statCard("hero-credit-card", "Taxable value", rupees(s.subtotal)) +
          UI.statCard("hero-check-circle", "Tax collected", rupees(s.tax_total),
                      "CGST + SGST + IGST");

        // Last 14 billed days: beyond that the bars are too narrow to read, and a till
        // looking at a year wants the table below, not 300 slivers.
        var days = report.daily.slice(-14);
        var peak = days.reduce(function (acc, d) {
          return Math.max(acc, d.cgst_sgst, d.igst);
        }, 0);

        $("an-chart").innerHTML = UI.barChart(
          days.map(function (d) {
            return { label: dayLabel(d.date), a: d.cgst_sgst, b: d.igst };
          }),
          niceCeiling(peak),
          function (tick) {
            return "₹" + Math.round(tick).toLocaleString("en-IN");
          }
        );

        $("an-payments").innerHTML = UI.donutChart(
          report.payments.map(function (p) {
            return {
              label: p.payment_type.toUpperCase(),
              value: p.grand_total,
              display: rupees(p.grand_total),
            };
          }),
          String(s.invoice_count),
          s.invoice_count === 1 ? "invoice" : "invoices"
        );

        $("an-top").innerHTML = report.top_items
          .map(function (row) {
            return (
              '<tr class="border-b border-base-300 text-sm last:border-0 hover:bg-base-200/60">' +
              '<td class="py-2 font-mono">' + escapeHtml(row.item_code) + "</td>" +
              '<td class="py-2">' + escapeHtml(row.description) + "</td>" +
              '<td class="py-2 text-right font-mono tabular-nums">' + money(row.qty) + "</td>" +
              '<td class="py-2 text-right font-mono tabular-nums">' + money(row.revenue) +
              "</td>" +
              "</tr>"
            );
          })
          .join("");

        status(s.invoice_count + " invoice(s) · " + rupees(s.grand_total) + " billed.");
      })
      .catch(function (err) {
        status("analytics failed: " + errText(err));
      });
  }

  $("an-range").addEventListener("change", loadAnalytics);


  /* ----------------------------------------------------------------------- sync */

  /** The last status the worker reported, so the badge and the Sync page agree. */
  var syncState = null;

  /** Stops the badge polling once signed out. */
  var syncTimer = null;

  /**
   * The badge's three readings. Colour alone does not tell a cashier whether anything is
   * waiting, so each one carries its words too.
   */
  function paintBadge(view) {
    var icon = $("conn-icon");
    var label = $("conn-label");
    var badge = $("conn-badge");

    // Written out rather than assembled, so Tailwind can see every class this can produce.
    var TONES = {
      ok: "text-success",
      warn: "text-warning",
      bad: "text-error",
      idle: "text-base-content/45",
    };

    var tone;
    var text;
    var title;

    if (!view) {
      tone = "idle";
      text = "Checking…";
      title = "Reading sync status";
    } else if (view.token_rejected) {
      // The one state that needs somebody to do something. It is not "offline" — the
      // back office answered, and what it said was no.
      tone = "bad";
      text = "Sync disabled — invalid token";
      title = view.last_error || "The back office rejected this till's token";
    } else if (view.configured && !view.token_set) {
      tone = "idle";
      text = "Not registered";
      title = "No token for this till yet — Settings › Sync";
    } else if (!view.configured) {
      // Not an error. A till nobody has pointed at a back office yet is waiting to be
      // set up, and a red badge on a fresh install would be a lie.
      tone = "idle";
      text = "Not linked";
      title = "No back office address set — Settings › Sync";
    } else if (view.healthy) {
      tone = "ok";
      text = "Connected";
      title = "Last sent " + (view.last_success || "just now");
    } else {
      tone = "warn";
      text = view.pending > 0 ? "Offline — " + view.pending + " pending" : "Offline";
      title = view.last_error || "Waiting to reach the back office";
    }

    icon.className = "hero-cloud size-4 shrink-0 " + TONES[tone];
    label.className = TONES[tone];
    label.textContent = text;
    badge.title = title;
  }

  /** Reads the worker's state and repaints the badge. Cheap, and never blocks billing. */
  function refreshSync() {
    if (!invoke || !user) return Promise.resolve(null);

    return invoke("sync_status")
      .then(function (view) {
        syncState = view;
        paintBadge(view);
        // Repaint the Sync page too, if that is what is open.
        if (document.getElementById("pane-sync").classList.contains("is-active")) {
          renderSyncPage(view);
        }
        return view;
      })
      .catch(function (err) {
        syncState = null;
        paintBadge(null);
        $("conn-badge").title = errText(err);
        return null;
      });
  }

  /** Polls the badge on the worker's own cadence, so the two never drift far apart. */
  function startSyncPolling() {
    stopSyncPolling();
    refreshSync().then(function (view) {
      var seconds = view && view.poll_seconds ? view.poll_seconds : 10;
      syncTimer = setInterval(refreshSync, seconds * 1000);
    });
  }

  function stopSyncPolling() {
    if (syncTimer) clearInterval(syncTimer);
    syncTimer = null;
  }

  /** `2026-09-13 11:32:04` → `11:32:04 today`, or the date when it is older. */
  function whenSynced(stamp) {
    if (!stamp) return "never";
    var parts = String(stamp).split(" ");
    if (parts.length !== 2) return stamp;
    var today = new Date();
    var iso =
      today.getFullYear() +
      "-" +
      String(today.getMonth() + 1).padStart(2, "0") +
      "-" +
      String(today.getDate()).padStart(2, "0");
    return parts[0] === iso ? parts[1] + " today" : parts[1] + " on " + dateOnly(parts[0]);
  }

  function renderSyncPage(view) {
    if (!view) {
      rowList($("sync-rows"), [["Status", "Could not read the sync worker"]]);
      return;
    }

    var state;
    if (view.token_rejected) {
      state = "Sync disabled — invalid token";
    } else if (!view.configured) {
      state = "Not linked — no address set";
    } else if (!view.token_set) {
      state = "Not registered — no token entered";
    } else if (view.healthy) {
      state = "Connected";
    } else {
      state = "Offline — retrying";
    }

    var rows = [
      ["Status", state],
      ["This till", view.node_id],
      ["Waiting to send", view.pending === 0 ? "Nothing — everything is up to date"
                                             : view.pending + " record(s)"],
      ["Last sent", whenSynced(view.last_success)],
    ];

    rows.push([
      "Token",
      view.token_rejected
        ? "Rejected by the back office " + (view.token_hint || "")
        : view.token_set
          ? "Set " + (view.token_hint || "")
          : "None entered yet",
    ]);

    if (view.last_batch > 0 && view.last_success) {
      rows.push(["Last batch", view.last_batch + " record(s)"]);
    }
    if (view.last_error) {
      rows.push(["Last problem", view.last_error]);
    }
    if (view.token_rejected) {
      // Deliberately not "retrying": the worker has stopped, and saying otherwise would
      // leave somebody waiting for a recovery that is never coming on its own.
      rows.push(["Retrying", "Stopped — waiting for a working token"]);
    } else if (view.consecutive_failures > 0) {
      rows.push([
        "Retrying",
        "attempt " + (view.consecutive_failures + 1) + ", in about " +
          view.retry_in_seconds + "s",
      ]);
    }

    rowList($("sync-rows"), rows);

    // Do not fight somebody who is mid-edit in the address box.
    var box = $("sync-endpoint");
    if (document.activeElement !== box) box.value = view.endpoint || "";

    // The token box stays empty: the stored value is never sent back to the screen, so
    // there is nothing to put in it. Typing replaces what is stored.
    var alert = $("token-alert");
    if (view.token_rejected) {
      alert.textContent =
        "The back office rejected this till's token. Nothing has been lost — every " +
        "invoice is still queued here and will be sent as soon as a working token is " +
        "entered below.";
      alert.classList.remove("hidden");
    } else {
      alert.classList.add("hidden");
    }
  }

  function syncNow() {
    if (!invoke) return bridgeMissing("sync_now");
    status("Sending…");
    invoke("sync_now")
      .then(function () {
        // The command returns as soon as the worker is nudged, deliberately: the button
        // must not hang the screen on a network round trip. Look again once it has had a
        // moment to try.
        setTimeout(refreshSync, 1200);
        setTimeout(refreshSync, 3000);
      })
      .catch(function (err) {
        status(errText(err));
      });
  }

  function saveEndpoint(event) {
    if (event) event.preventDefault();
    if (!invoke) return bridgeMissing("set_sync_endpoint");

    var button = $("endpoint-save");
    button.disabled = true;
    setMsg("endpoint-msg", "Saving…", false);

    invoke("set_sync_endpoint", { endpoint: $("sync-endpoint").value })
      .then(function (saved) {
        button.disabled = false;
        setMsg("endpoint-msg", saved ? "Saved." : "Cleared.", false);
        status(saved ? "Back office address saved." : "Back office address cleared.");
        setTimeout(refreshSync, 1200);
        setTimeout(refreshSync, 3000);
      })
      .catch(function (err) {
        button.disabled = false;
        setMsg("endpoint-msg", errText(err), true);
      });
  }

  function saveToken(event) {
    if (event) event.preventDefault();
    if (!invoke) return bridgeMissing("set_sync_token");

    var box = $("sync-token");
    var button = $("token-save");
    button.disabled = true;
    setMsg("token-msg", "Saving…", false);

    invoke("set_sync_token", { token: box.value })
      .then(function (view) {
        button.disabled = false;
        // Cleared straight away. The value is stored; leaving it on screen only leaves a
        // credential sitting in a text box for whoever walks past the counter next.
        box.value = "";
        setMsg("token-msg", view.token_set ? "Saved." : "Cleared.", false);
        status(view.token_set ? "Token saved for this till." : "Token cleared.");
        syncState = view;
        paintBadge(view);
        renderSyncPage(view);
        setTimeout(refreshSync, 1500);
        setTimeout(refreshSync, 3500);
      })
      .catch(function (err) {
        button.disabled = false;
        setMsg("token-msg", errText(err), true);
      });
  }

  $("token-form").addEventListener("submit", saveToken);
  $("sync-now").addEventListener("click", syncNow);
  $("endpoint-form").addEventListener("submit", saveEndpoint);
  // The badge is a shortcut to the page that explains it.
  $("conn-badge").addEventListener("click", function () {
    showPane("sync");
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
      if ($("login-screen").hidden) printAndLock();
    }
  });

  renderCustomer();
  renderRows();
  loadTheme();
  bootstrap();
})();
