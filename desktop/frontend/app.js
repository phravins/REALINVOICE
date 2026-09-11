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
    if (name !== "billing") status(name + ": coming soon.");
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

    if (!rows.length) {
      body.innerHTML = "";
      return;
    }

    body.innerHTML = rows
      .map(function (row, index) {
        return (
          "<tr>" +
          '<td class="mono">' + escapeHtml(row.item_code) + "</td>" +
          "<td>" + escapeHtml(row.description) + "</td>" +
          '<td class="num"><input class="qty-input" type="number" min="0" step="any" ' +
          'value="' + row.qty + '" data-index="' + index + '" ' +
          'aria-label="Quantity for ' + escapeHtml(row.item_code) + '" /></td>' +
          '<td class="num mono">' + money(row.rate) + "</td>" +
          '<td class="num mono">' + money(row.tax_rate) + "</td>" +
          '<td class="num mono" data-total="' + index + '">' + money(row.total) + "</td>" +
          '<td class="num"><button class="row-del" data-remove="' + index + '" ' +
          'title="Remove row" aria-label="Remove ' + escapeHtml(row.item_code) + '">×</button></td>' +
          "</tr>"
        );
      })
      .join("");
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
      var cell = document.querySelector('[data-total="' + index + '"]');
      if (cell) cell.textContent = money(total);
    });

    $("t-subtotal").textContent = rupees(quote.subtotal);
    $("t-cgst").textContent = rupees(quote.cgst);
    $("t-sgst").textContent = rupees(quote.sgst);
    $("t-igst").textContent = rupees(quote.igst);

    // Intra-state bills CGST + SGST; inter-state bills IGST. Never both.
    $("row-cgst").hidden = !quote.intra_state;
    $("row-sgst").hidden = !quote.intra_state;
    $("row-igst").hidden = quote.intra_state;

    $("t-grand").textContent = rupees(quote.grand_total);

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
      var cell = document.querySelector('[data-total="' + index + '"]');
      if (cell) cell.textContent = money(line.line_total);
      if (rows[index]) rows[index].total = line.line_total;
    });
    $("t-subtotal").textContent = rupees(invoice.subtotal);
    $("t-cgst").textContent = rupees(invoice.cgst);
    $("t-sgst").textContent = rupees(invoice.sgst);
    $("t-igst").textContent = rupees(invoice.igst);
    $("t-grand").textContent = rupees(invoice.grand_total);

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
