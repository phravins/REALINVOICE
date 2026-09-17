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

  /**
   * Every price list, and the one the bill on screen is being priced from.
   *
   * `activeList` is the attached customer's list, or the default when nobody is
   * attached — which is what a walk-in would pay, and what the picker quotes before
   * anyone is resolved. It is a cache of a decision core owns: the save re-resolves from
   * the stored customer and refuses if the two disagree.
   */
  var priceLists = [];
  var activeList = null;

  /**
   * The whole-bill discount, and the ceiling a cashier may reach without an owner.
   *
   * Both are only ever *displayed* from here. The discount is sent to core with the save
   * and re-applied there, and the ceiling is enforced in Rust — a gate the frontend can
   * open is not a gate.
   */
  var invoiceDiscount = { type: "none", value: 0 };
  var discountLimitPct = 15;

  /** What the attached customer already owes, for the badge and the credit-limit check. */
  var customerBalance = 0;

  function listName(id) {
    var match = priceLists.filter(function (l) { return l.id === id; })[0];
    return match ? match.name : "";
  }

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
  /**
   * The totals block, on the billing card and on a saved invoice alike.
   *
   * When anything was discounted the panel shows the whole working — what it would have
   * cost, what came off, and what tax was then charged on. Collapsing the discount into
   * the subtotal would leave a cashier unable to answer "why is this not the shelf
   * price", and would leave an auditor unable to answer it at all.
   */
  function renderTotals(el, t) {
    function row(label, value, style) {
      var grand = style === "grand";
      var discount = style === "discount";
      var taxable = style === "taxable";
      return (
        '<div class="flex items-baseline justify-between gap-4 ' +
        (grand
          ? 'mt-2 border-t border-base-300 pt-3"><dt class="text-base font-semibold">'
          : taxable
            ? 'border-t border-base-300 pt-2 mt-1 py-1"><dt class="text-sm font-medium">'
            : 'py-1"><dt class="text-sm ' +
              (discount ? "text-error" : "text-base-content/60") + '">') +
        label +
        "</dt><dd class=" +
        (grand
          ? '"font-mono text-2xl font-semibold tabular-nums">'
          : discount
            ? '"font-mono text-sm tabular-nums text-error">'
            : '"font-mono text-sm tabular-nums">') +
        (discount ? "−" : "") +
        rupees(value) +
        "</dd></div>"
      );
    }

    var discounted = t.discount_amount > 0;

    // With a discount the first line is what it would have cost; without one there is
    // nothing to distinguish, so it stays the plain "Subtotal" it has always been.
    var html = row(discounted ? "Subtotal (before discount)" : "Subtotal",
                   discounted ? t.pre_discount_subtotal : t.subtotal);
    if (discounted) {
      html += row("Discount", t.discount_amount, "discount");
      html += row("Taxable Value", t.subtotal, "taxable");
    }
    if (t.intra_state) {
      html += row("CGST", t.cgst) + row("SGST", t.sgst);
    } else {
      html += row("IGST", t.igst);
    }
    el.innerHTML = html + row("Grand Total", t.grand_total, "grand");
  }

  /** A saved invoice's totals, in the shape renderTotals expects. */
  function totalsOf(invoice) {
    return {
      // A stored invoice carries the split itself: IGST is only ever set inter-state.
      intra_state: !(invoice.igst > 0),
      // Read from the row as stored, never recomputed — which is what makes reopening a
      // discounted invoice show the breakdown it was billed with.
      pre_discount_subtotal: invoice.pre_discount_subtotal,
      discount_amount: invoice.discount_amount,
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

        // The discount control lives under the Rate, collapsed to a single icon until
        // the row actually has one. Most rows never do, and a discount box on every line
        // is a discount box nobody reads.
        var discounted = line.discount_type && line.discount_type !== "none" &&
                         line.discount_value > 0;
        var discountCell = "";
        if (editable) {
          discountCell =
            '<button type="button" data-discount="' + index + '" ' +
            'title="Discount this line" aria-label="Discount ' +
            escapeHtml(line.item_code) + '" class="mt-0.5 flex items-center gap-1 ' +
            'text-2xs ' + (discounted ? "text-error" : "text-base-content/45") +
            ' transition-colors hover:text-base-content">' +
            '<span class="hero-receipt-percent size-3.5" aria-hidden="true"></span>' +
            (discounted ? escapeHtml(discountLabel(line)) : "Discount") +
            "</button>";
        } else if (discounted) {
          // A flat discount's label is the same number as its amount, so it is only
          // spelled out for a percentage, where the two genuinely differ.
          discountCell =
            '<span class="mt-0.5 block text-2xs text-error">−' +
            money(line.discount_amount) +
            (line.discount_type === "percentage"
              ? " (" + escapeHtml(discountLabel(line)) + ")"
              : "") +
            "</span>";
        }

        var num = ' class="py-2 text-right font-mono tabular-nums"';

        // A one-off's item code is a generated key, not something anyone reads out. The
        // cell carries a tag instead, which is also the visual mark that this line came
        // off the counter rather than out of the catalogue.
        var code = line.custom
          ? '<span class="rounded-field bg-base-300 px-1.5 py-0.5 text-2xs font-medium ' +
            'uppercase tracking-wider text-base-content/70">custom</span>'
          : '<span class="font-mono">' + escapeHtml(line.item_code) + "</span>";

        return (
          '<tr class="border-b border-base-300 text-sm last:border-0 hover:bg-base-200/60">' +
          '<td class="py-2">' + code + "</td>" +
          '<td class="py-2">' + escapeHtml(line.description) + "</td>" +
          '<td class="py-2 text-right">' + qty + "</td>" +
          '<td class="py-2 text-right font-mono tabular-nums">' + money(line.rate) +
          (discountCell ? '<span class="block font-sans">' + discountCell + "</span>" : "") +
          "</td>" +
          "<td" + num + ">" + money(line.tax_rate) + "</td>" +
          '<td class="py-2 text-right font-mono tabular-nums" data-total="' + index +
          '">' + money(line.total) + "</td>" +
          remove +
          "</tr>"
        );
      })
      .join("");
  }

  /** Paise, the way core rounds. Only ever used to subtract two figures core produced. */
  function gstRound(value) {
    return Math.round(value * 100) / 100;
  }

  /** `partially_paid` -> "Partially paid". */
  function statusWords(status) {
    return {
      unpaid: "Unpaid",
      partially_paid: "Partially paid",
      paid: "Paid",
    }[status] || status;
  }

  /**
   * A pill for an invoice's payment status.
   *
   * "Paid" gets no pill: it is the ordinary case, and a column of green ticks buries the
   * two rows somebody is actually looking for.
   */
  function paymentStatusBadge(invoice) {
    if (invoice.payment_status === "paid") return "";
    var tone =
      invoice.payment_status === "unpaid"
        ? "bg-warning/15 text-warning"
        : "bg-info/15 text-info";
    return (
      '<span class="ml-1 rounded-field px-1.5 py-0.5 text-2xs font-medium uppercase ' +
      'tracking-wider ' + tone + '">' + escapeHtml(statusWords(invoice.payment_status)) +
      "</span>"
    );
  }

  /** "10%" or "₹500" — how a line's discount was expressed, not what it came to. */
  function discountLabel(line) {
    if (line.discount_type === "percentage") return money(line.discount_value) + "%";
    if (line.discount_type === "flat") return "₹" + money(line.discount_value);
    return "";
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
    } else if (name === "customers") {
      // Back to the list on every visit: a ledger left open from last time is somebody
      // else's account staring at whoever opens the tab next.
      closeLedger();
      status("Who owes what.");
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

  /* --------------------------------------------------------------- price lists */

  /**
   * Loads the lists once per sign-in, and settles on the default.
   *
   * Everything that draws a list dropdown reads `priceLists`, so they cannot disagree
   * about what exists or what it is called.
   */
  function loadPriceLists() {
    if (!invoke) return Promise.resolve([]);

    return invoke("price_lists")
      .then(function (lists) {
        priceLists = lists || [];
        if (!activeList) {
          activeList = priceLists.filter(function (l) { return l.is_default; })[0] || null;
        }
        renderPriceListOptions();
        renderPriceListRows();
        return priceLists;
      })
      .catch(function (err) {
        status("price_lists failed: " + errText(err));
        return [];
      });
  }

  /** Fills every <select> that offers a price list. */
  function renderPriceListOptions() {
    var options = priceLists
      .map(function (list) {
        return (
          '<option value="' + list.id + '">' + escapeHtml(list.name) +
          (list.is_default ? " (default)" : "") + "</option>"
        );
      })
      .join("");

    // "Default" is offered as its own choice rather than pre-selecting today's default:
    // a customer left on it follows the flag if the shop moves it later.
    $("nc-pricelist").innerHTML =
      '<option value="">Default</option>' + options;
    $("pricing-select").innerHTML = '<option value="">Default</option>' + options;
  }

  /**
   * Switches the bill to a different price list and re-rates what is already on it.
   *
   * This is the case the toast exists for. Items get added before the customer is
   * resolved all the time — the bag of cement is on the counter before the phone number
   * is — and those rows were priced at the default. Attaching a wholesale buyer has to
   * change them, and a total that changes silently is how a counter ends up arguing
   * about a printed bill.
   */
  function applyPriceList(list, reason) {
    var previous = activeList;
    activeList = list || null;
    renderCustomer();

    var changedList = !previous || !activeList || previous.id !== activeList.id;
    if (!activeList || !rows.length || !changedList) {
      renderCustomer();
      return Promise.resolve(0);
    }
    if (!invoke) return Promise.resolve(0);

    return invoke("resolve_rates", {
      itemIds: rows.map(function (row) { return row.item_id; }),
      priceListId: activeList.id,
    })
      .then(function (resolved) {
        var moved = 0;
        resolved.forEach(function (entry, index) {
          var row = rows[index];
          if (!row || row.rate === entry.rate) return;
          row.rate = entry.rate;
          row.from_price_list = entry.from_price_list;
          moved += 1;
        });

        if (moved) {
          renderRows();
          requote();
          UI.toast(
            "Prices updated for " + (reason || activeList.name) +
              " — " + activeList.name + " rates on " + moved + " line(s).",
            "info"
          );
        }
        return moved;
      })
      .catch(function (err) {
        status("resolve_rates failed: " + errText(err));
        return 0;
      });
  }

  /** Follows whoever is attached; falls back to the default when nobody is. */
  function syncPriceListToCustomer() {
    if (!invoke) return Promise.resolve(0);

    if (!customer) {
      var fallback = priceLists.filter(function (l) { return l.is_default; })[0];
      return applyPriceList(fallback || activeList, "a walk-in");
    }
    return invoke("price_list_for_customer", { customerId: customer.id })
      .then(function (list) {
        return applyPriceList(list, customer.name);
      })
      .catch(function (err) {
        status("price_list_for_customer failed: " + errText(err));
        return 0;
      });
  }

  /* ------------------------------------------------------------------ customer */

  /**
   * Draws the Customer section from `customer`. Exactly one of three boxes is on screen
   * at a time — the mobile search, the attached customer, or the "nobody matched"
   * prompt — so there is never a question of which one the operator is looking at.
   */
  function renderCustomer() {
    var attached = !!customer;

    $("customer-chip").hidden = !attached;
    $("customer-search").hidden = attached || locked;
    // Detaching is an edit, and a saved invoice is not editable.
    $("customer-detach").hidden = locked;
    if (attached) $("customer-miss").hidden = true;

    renderPricing();

    if (!attached) {
      $("customer-line").innerHTML = "";
      $("customer-outstanding").hidden = true;
      return;
    }

    // "Billing to:" rather than a bare name. This line stays up for the whole bill, so it
    // has to read as a standing statement about the invoice, not as a search result that
    // happens to still be on screen.
    $("customer-line").innerHTML =
      '<span class="text-base-content/60">Billing to:</span> ' +
      '<span class="font-medium text-base-content">' + escapeHtml(customer.name) + "</span>" +
      ' <span class="text-base-content/60">· GSTIN ' +
      escapeHtml(customer.gstin || "unregistered") +
      "</span>" +
      ' <span class="text-base-content/45">· ' + escapeHtml(customer.mobile) +
      " · place of supply " + escapeHtml(customer.place_of_supply) + "</span>";
  }

  /**
   * The "Pricing: Wholesale" marker beside the customer.
   *
   * An owner gets a dropdown — the place you notice the wrong list is the place you
   * should be able to fix it — and everyone else the same thing as plain text. The
   * command behind it refuses a cashier regardless.
   */
  function renderPricing() {
    var badge = $("pricing-badge");
    var select = $("pricing-select");
    var owner = !!user && user.role === "owner";
    var editable = owner && !!customer && !locked;

    if (!activeList) {
      badge.hidden = true;
      select.hidden = true;
      return;
    }

    badge.hidden = editable;
    select.hidden = !editable;

    badge.textContent = "Pricing: " + activeList.name;
    badge.title = customer
      ? customer.name + " is billed from the " + activeList.name + " price list."
      : "No customer attached — pricing from the " + activeList.name + " list.";

    if (editable) {
      // The customer's own assignment, not the list in force: "Default" has to stay
      // distinguishable from "assigned to the list that happens to be default".
      select.value = customer.price_list_id == null ? "" : String(customer.price_list_id);
      select.title = badge.title;
    }
  }

  function setCustomer(found) {
    customer = found;
    hideNewCustomer();
    $("customer-miss").hidden = true;
    renderCustomer();
    requote();
    // Their list may not be the one the rows on screen were priced at.
    syncPriceListToCustomer();
    loadOutstanding();
  }

  /** What they already owe, shown beside their name before anyone extends more credit. */
  function loadOutstanding() {
    var badge = $("customer-outstanding");
    if (!invoke || !customer) {
      badge.hidden = true;
      customerBalance = 0;
      return Promise.resolve(0);
    }

    return invoke("customer_balance", { customerId: customer.id })
      .then(function (balance) {
        customerBalance = balance;
        if (!customer) return balance;

        if (balance > 0) {
          badge.textContent = "Outstanding: " + rupees(balance);
          badge.className =
            "shrink-0 rounded-field bg-warning/15 px-2 py-0.5 text-2xs font-medium text-warning";
          badge.hidden = false;
        } else if (balance < 0) {
          // The shop is holding their money. Worth saying, for the same reason a debt is.
          badge.textContent = "In credit: " + rupees(-balance);
          badge.className =
            "shrink-0 rounded-field bg-success/15 px-2 py-0.5 text-2xs font-medium text-success";
          badge.hidden = false;
        } else {
          badge.hidden = true;
        }
        return balance;
      })
      .catch(function (err) {
        badge.hidden = true;
        status("customer_balance failed: " + errText(err));
        return 0;
      });
  }

  /** Puts the bill back to nobody attached, ready to search again. */
  function detachCustomer() {
    if (locked) return;
    var was = customer;
    customer = null;
    $("customer-miss").hidden = true;
    hideNewCustomer();
    renderCustomer();
    requote();
    // Back to what a walk-in would pay, and the rows follow.
    syncPriceListToCustomer();
    loadOutstanding();
    $("mobile-input").value = "";
    $("mobile-input").focus();
    status(was ? "Detached " + was.name + "." : "Customer detached.");
  }

  /**
   * Brings the Customer section into view and puts the caret in the mobile box.
   *
   * This is what "Attach a customer first" does when clicked. An error that names a
   * requirement but leaves the operator hunting for where to satisfy it is the bug this
   * whole section exists to fix, so the message is a way there, not just a complaint.
   */
  function focusCustomerSection() {
    var section = $("customer-section");
    section.scrollIntoView({ behavior: "smooth", block: "nearest" });

    // A brief ring, because the section may well have been on screen the whole time and
    // a silent scroll of nought pixels would look like nothing happened.
    section.classList.add("rounded-box", "ring-2", "ring-warning", "ring-offset-4",
                          "ring-offset-base-100");
    window.setTimeout(function () {
      section.classList.remove("rounded-box", "ring-2", "ring-warning", "ring-offset-4",
                               "ring-offset-base-100");
    }, 1200);

    if (!$("new-customer").hidden) $("nc-name").focus();
    else $("mobile-input").focus();
  }

  function searchCustomer() {
    var mobile = $("mobile-input").value.trim();
    if (!mobile) {
      $("mobile-input").focus();
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
          return;
        }
        customer = null;
        hideNewCustomer();
        renderCustomer();
        // The miss is not an error. It is the second most common thing that happens at a
        // counter, so it gets an answer on the spot.
        $("customer-miss-text").textContent = "No customer with " + mobile + " yet.";
        $("customer-miss").hidden = false;
        status("No customer for " + mobile + " — add one below.");
      })
      .catch(function (err) {
        status("search_customer failed: " + errText(err));
      });
  }

  /* -------------------------------------------------------------- new customer */

  function showNewCustomer(mobile) {
    $("nc-name").value = "";
    $("nc-mobile").value = mobile || $("mobile-input").value.trim();
    $("nc-gstin").value = "";
    $("nc-pos").value = "TN";
    $("nc-pricelist").value = "";
    setMsg("nc-msg", "", false);
    $("customer-miss").hidden = true;
    $("new-customer").hidden = false;
    $("nc-name").focus();
  }

  function hideNewCustomer() {
    $("new-customer").hidden = true;
  }

  function createCustomer() {
    var payload = {
      name: $("nc-name").value.trim(),
      mobile: $("nc-mobile").value.trim(),
      gstin: $("nc-gstin").value.trim() || null,
      place_of_supply: $("nc-pos").value.trim().toUpperCase(),
      price_list_id: $("nc-pricelist").value ? Number($("nc-pricelist").value) : null,
    };

    if (!payload.name || !payload.mobile || !payload.place_of_supply) {
      return setMsg("nc-msg", "Name, mobile and place of supply are required.", true);
    }
    if (!invoke) return bridgeMissing("create_customer");

    setMsg("nc-msg", "Saving…", false);
    invoke("create_customer", { payload: payload })
      .then(function (created) {
        // Attached straight away: whoever filled this in is mid-sale, and making them
        // search for the number they just typed would be absurd.
        $("mobile-input").value = created.mobile;
        setCustomer(created);
        status("Registered " + created.name + " (#" + created.id + ") and attached.");
      })
      .catch(function (err) {
        setMsg("nc-msg", errText(err), true);
      });
  }

  /* ---------------------------------------------------------------- item rows */

  function renderRows() {
    var body = $("items-body");
    $("empty-rows").hidden = rows.length > 0;
    body.innerHTML = rows.length ? lineRowsHtml(rows, !locked) : "";
  }

  /**
   * Puts an item on the bill.
   *
   * `rate` is the resolved rate for the active price list, not `item.rate` — the base
   * rate is a fallback inside core, not something this screen should be reading.
   */
  function addRow(item, rate, fromPriceList) {
    var resolved = rate == null ? item.rate : rate;
    rows.push({
      item_id: item.id,
      item_code: item.item_code,
      description: item.description,
      uom: item.uom,
      qty: 1,
      rate: resolved,
      tax_rate: item.tax_rate,
      total: resolved, // provisional; core's quote overwrites it
      custom: !!item.custom,
      from_price_list: !!fromPriceList,
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
    var discount = event.target.closest("[data-discount]");
    if (discount) {
      openLineDiscount(Number(discount.dataset.discount));
      return;
    }
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
    closeOneOff();
    runItemSearch("");
    $("item-query").focus();
  }

  function closePicker() {
    $("item-picker").hidden = true;
    closeOneOff();
  }

  function runItemSearch(query) {
    var list = $("picker-results");
    if (!invoke) {
      list.innerHTML =
        '<li class="px-3 py-2 text-sm text-error">Tauri bridge unavailable.</li>';
      return;
    }

    invoke("search_item", {
      query: query,
      priceListId: activeList ? activeList.id : null,
    })
      .then(function (items) {
        // The offer is only ever the way out of an empty result. Showing it alongside
        // matches would invite a duplicate one-off of something already in stock.
        offerOneOff(items.length === 0 ? query.trim() : "");

        if (!items.length) {
          list.innerHTML = query.trim()
            ? '<li class="px-3 py-2 text-sm text-base-content/60">No catalogue item ' +
              "matches “" + escapeHtml(query.trim()) + "”.</li>"
            : '<li class="px-3 py-2 text-sm text-base-content/60">No items match.</li>';
          return;
        }
        // `items` are PricedItems: the rate shown is the rate this customer will be
        // billed, resolved by core for the active list rather than the sticker price.
        list.innerHTML = items
          .map(function (priced, index) {
            var item = priced.item;
            return (
              '<li><button type="button" data-pick="' + index + '" ' +
              'class="flex w-full items-baseline gap-3 rounded-field px-3 py-2 ' +
              'text-left text-sm transition-colors hover:bg-base-200">' +
              '<span class="w-32 shrink-0 font-mono">' + escapeHtml(item.item_code) +
              "</span>" +
              '<span class="min-w-0 flex-1 truncate">' + escapeHtml(item.description) +
              "</span>" +
              (priced.from_price_list
                ? '<span class="shrink-0 rounded-field bg-base-300 px-1.5 py-0.5 ' +
                  'text-2xs font-medium uppercase tracking-wider text-base-content/70">' +
                  escapeHtml(activeList ? activeList.name : "list") + "</span>"
                : "") +
              '<span class="shrink-0 font-mono text-xs text-base-content/45">₹' +
              money(priced.rate) + " · " + money(item.tax_rate) + "% · " +
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

  /* ------------------------------------------------------------ one-off items */

  /**
   * Shows or hides the "add this as a one-off" offer.
   *
   * A one-off is billed but never filed: core gives it a row so the invoice line has
   * something to point at, flagged `custom` so it stays out of the catalogue that the
   * shop actually maintains. Typing a repair charge onto a bill must not quietly grow
   * the Inventory list by one item a day.
   */
  function offerOneOff(text) {
    var offer = $("oneoff-offer");
    if (!text || !$("oneoff-form").hidden) {
      offer.hidden = true;
      return;
    }
    $("oneoff-offer-text").textContent = "Add “" + text + "” as a one-off item";
    offer.hidden = false;
  }

  function openOneOff(description) {
    $("oneoff-offer").hidden = true;
    $("oo-desc").value = description || "";
    $("oo-rate").value = "";
    $("oo-tax").value = "18";
    $("oo-qty").value = "1";
    $("oo-uom").value = "NOS";
    setMsg("oo-msg", "", false);
    $("oneoff-form").hidden = false;
    $("oo-rate").focus();
  }

  function closeOneOff() {
    $("oneoff-form").hidden = true;
    $("oneoff-offer").hidden = true;
  }

  function addOneOff(event) {
    if (event) event.preventDefault();
    if (!invoke) return bridgeMissing("create_custom_item");

    var description = $("oo-desc").value.trim();
    var rate = parseFloat($("oo-rate").value);
    var taxRate = parseFloat($("oo-tax").value);
    var qty = parseFloat($("oo-qty").value);

    // Shape only. The amounts themselves are still core's to validate and to price —
    // nothing here works out what the line costs.
    if (!description) return setMsg("oo-msg", "Enter a description.", true);
    if (!isFinite(rate) || rate < 0) return setMsg("oo-msg", "Enter a rate.", true);
    if (!isFinite(taxRate) || taxRate < 0 || taxRate > 100) {
      return setMsg("oo-msg", "Tax % must be between 0 and 100.", true);
    }
    if (!isFinite(qty) || qty <= 0) return setMsg("oo-msg", "Enter a quantity.", true);

    setMsg("oo-msg", "Adding…", false);
    invoke("create_custom_item", {
      description: description,
      rate: rate,
      taxRate: taxRate,
      uom: $("oo-uom").value.trim(),
    })
      .then(function (item) {
        // A one-off is priced at what was just typed on every list, so its rate is its
        // own and there is nothing to resolve.
        addRow(item, item.rate, false);
        // The quantity is part of the same entry, so it is applied rather than left at
        // the 1 that `addRow` assumes.
        rows[rows.length - 1].qty = qty;
        renderRows();
        requote();
        closePicker();
        status("Added one-off “" + item.description + "”.");
      })
      .catch(function (err) {
        setMsg("oo-msg", errText(err), true);
      });
  }

  $("oneoff-open").addEventListener("click", function () {
    openOneOff($("item-query").value.trim());
  });
  $("oneoff-form").addEventListener("submit", addOneOff);
  $("oo-cancel").addEventListener("click", function () {
    closeOneOff();
    offerOneOff($("item-query").value.trim());
    $("item-query").focus();
  });

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
      addRow(picked.item, picked.rate, picked.from_price_list);
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
    setMsg("id-msg", "", false);

    // Row totals are core's line values, not a number this file worked out.
    quote.line_totals.forEach(function (total, index) {
      if (rows[index]) rows[index].total = total;
      var cell = document.querySelector('#items-body [data-total="' + index + '"]');
      if (cell) cell.textContent = money(total);
    });
    (quote.line_discounts || []).forEach(function (amount, index) {
      if (rows[index]) rows[index].discount_amount = amount;
    });

    // What the shop's limit makes of this bill, before the save says so.
    var pct = quote.pre_discount_subtotal > 0
      ? (quote.discount_amount / quote.pre_discount_subtotal) * 100
      : 0;
    $("id-limit").textContent =
      quote.discount_amount > 0
        ? money(pct) + "% of the bill" + (needsApproval(pct) ? " · needs owner approval" : "")
        : "Cashiers may discount up to " + money(discountLimitPct) + "%";
    $("id-limit").className = needsApproval(pct)
      ? "text-2xs text-error"
      : "text-2xs text-base-content/45";

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
    actionMsg("", false);
  }

  /**
   * The line under the Print & Lock button.
   *
   * `fix` is an optional { label, run } pair rendered as a button beside the text, for
   * errors that name something the operator can put right from here.
   */
  function actionMsg(text, isError, fix) {
    var msg = $("action-msg");
    msg.className = isError ? "text-sm text-error" : "text-sm text-base-content/60";
    msg.textContent = text || "";

    if (!text || !fix) return;

    var button = document.createElement("button");
    button.type = "button";
    button.textContent = fix.label;
    button.className =
      "ml-2 rounded-field border border-current px-2 py-0.5 text-xs font-medium " +
      "underline-offset-2 hover:underline";
    button.addEventListener("click", fix.run);
    msg.appendChild(button);
  }

  /** Re-prices every row through core. Called on any change to rows or customer. */
  function requote() {
    clearMessage();
    if (!invoke) return bridgeMissing("quote_invoice");
    // Every command is gated on a session; there is nothing to price before sign-in.
    if (!user) return;

    // With no customer resolved yet, quote against the home state so the operator still
    // sees live figures; the split is re-quoted the moment one is attached. With one
    // attached, core uses *their* state and ignores this.
    var placeOfSupply = customer ? customer.place_of_supply : "TN";

    // Item and quantity and discount — no rates and no money. Core resolves the price
    // from the customer's list and does every sum, here exactly as it does on the save,
    // so the panel cannot show a figure the save would disagree with.
    var lines = rows.map(function (row) {
      return {
        item_id: row.item_id,
        qty: row.qty,
        rate: null,
        tax_rate: null,
        discount_type: row.discount_type || "none",
        discount_value: row.discount_value || 0,
      };
    });

    var seq = ++quoteSeq;
    invoke("quote_invoice", {
      customerId: customer ? customer.id : null,
      placeOfSupply: placeOfSupply,
      lines: lines,
      invoiceDiscountType: invoiceDiscount.type,
      invoiceDiscountValue: invoiceDiscount.value,
    })
      .then(function (quote) {
        if (seq !== quoteSeq) return; // a newer edit already won
        renderQuote(quote);
      })
      .catch(function (err) {
        // A discount core refuses — bigger than the bill, say — leaves the last good
        // figures on screen and says why, rather than blanking the panel.
        setMsg("id-msg", errText(err), true);
        status("quote_invoice failed: " + errText(err));
      });
  }

  /* ---------------------------------------------------------------- discounts */

  /** Whether the shop's limit puts this discount behind an owner, for a cashier. */
  function needsApproval(percent) {
    return !!user && user.role !== "owner" && percent > discountLimitPct + 1e-9;
  }

  /** Which row the discount editor is open on, or null. */
  var discountRow = null;

  function openLineDiscount(index) {
    var row = rows[index];
    if (!row || locked) return;

    discountRow = index;
    $("ld-item").textContent = row.item_code;
    $("ld-type").value = row.discount_type || "none";
    $("ld-value").value = row.discount_value || "";
    setMsg("ld-msg", "", false);
    $("line-discount").hidden = false;
    closePicker();
    $("ld-value").focus();
  }

  function closeLineDiscount() {
    discountRow = null;
    $("line-discount").hidden = true;
  }

  function applyLineDiscount(event) {
    if (event) event.preventDefault();
    var row = rows[discountRow];
    if (!row) return closeLineDiscount();

    var kind = $("ld-type").value;
    if (kind === "none") return clearLineDiscount();

    var value = parseFloat($("ld-value").value);
    // Shape only — whether it is bigger than the line is core's to say, and it says so
    // when the re-quote comes back.
    if (!isFinite(value) || value <= 0) {
      return setMsg("ld-msg", "Enter a discount above zero.", true);
    }
    if (kind === "percentage" && value > 100) {
      return setMsg("ld-msg", "A percentage cannot be more than 100.", true);
    }

    row.discount_type = kind;
    row.discount_value = value;
    closeLineDiscount();
    renderRows();
    requote();
    status("Discounted " + row.item_code + " by " + discountLabel(row) + ".");
  }

  function clearLineDiscount() {
    var row = rows[discountRow];
    if (row) {
      row.discount_type = "none";
      row.discount_value = 0;
      row.discount_amount = 0;
    }
    closeLineDiscount();
    renderRows();
    requote();
    if (row) status("Discount removed from " + row.item_code + ".");
  }

  $("line-discount").addEventListener("submit", applyLineDiscount);
  $("ld-clear").addEventListener("click", clearLineDiscount);
  $("ld-cancel").addEventListener("click", function () {
    closeLineDiscount();
    status("Discount unchanged.");
  });

  /** The whole-bill control. A type of "none" disables the value box rather than hiding
   *  it, so the row does not jump about as somebody cycles through the options. */
  function syncInvoiceDiscountControls() {
    var kind = $("id-type").value;
    $("id-value").disabled = kind === "none";
    if (kind === "none") $("id-value").value = "";
  }

  function applyInvoiceDiscount() {
    var kind = $("id-type").value;
    var raw = $("id-value").value.trim();
    var value = parseFloat(raw);

    if (kind === "none") {
      invoiceDiscount = { type: "none", value: 0 };
      setMsg("id-msg", "", false);
      return requote();
    }
    if (!raw) {
      // Mid-typing: hold the last good discount rather than flickering to zero.
      return;
    }
    if (!isFinite(value) || value < 0) {
      return setMsg("id-msg", "Enter a discount of zero or more.", true);
    }
    if (kind === "percentage" && value > 100) {
      return setMsg("id-msg", "A percentage cannot be more than 100.", true);
    }

    invoiceDiscount = { type: kind, value: value };
    setMsg("id-msg", "", false);
    requote();
  }

  $("id-type").addEventListener("change", function () {
    syncInvoiceDiscountControls();
    applyInvoiceDiscount();
  });
  $("id-value").addEventListener("input", applyInvoiceDiscount);

  /* ---------------------------------------------------------- owner approval */

  /** Resolved with the credentials, or rejected when the prompt is cancelled. */
  var approvalPending = null;

  /** Why a discount needs signing off, in words. */
  function discountReason(percent) {
    return (
      "This bill discounts " + money(percent) + "% of " +
      money(discountLimitPct) + "% allowed at the counter. An owner has to sign it off."
    );
  }

  /**
   * Why a credit sale needs signing off, or null when it does not.
   *
   * Only asked for a sale going on the account: a bill paid at the counter adds nothing
   * to what anyone owes, however large. Core checks again on the way to the save, so a
   * stale answer here cannot let one through.
   */
  function creditApprovalReason(grandTotal) {
    if (!invoke || !customer || !user || user.role === "owner") return Promise.resolve(null);
    if (selectedPayment() !== "credit") return Promise.resolve(null);

    return invoke("check_credit_limit", { customerId: customer.id, amount: grandTotal })
      .then(function (check) {
        if (!check.over_limit) return null;
        return (
          "This sale would take " + customer.name + " to " +
          rupees(check.balance + check.amount) + " against a " +
          rupees(check.credit_limit) + " credit limit. An owner has to approve it."
        );
      })
      .catch(function () {
        // The save re-checks, so a failure here costs a prompt, not the control.
        return null;
      });
  }

  function askOwnerApproval(reason) {
    $("approval-why").textContent = reason;
    $("ap-user").value = "";
    $("ap-pass").value = "";
    setMsg("ap-msg", "", false);
    $("approval-overlay").hidden = false;
    UI.enhancePasswordFields($("approval-form"));
    $("ap-user").focus();

    return new Promise(function (resolve, reject) {
      approvalPending = { resolve: resolve, reject: reject };
    });
  }

  function closeApproval() {
    $("approval-overlay").hidden = true;
    approvalPending = null;
  }

  $("approval-form").addEventListener("submit", function (event) {
    event.preventDefault();
    var username = $("ap-user").value.trim();
    var password = $("ap-pass").value;
    if (!username || !password) {
      return setMsg("ap-msg", "Enter the owner's username and password.", true);
    }
    var waiting = approvalPending;
    closeApproval();
    if (waiting) waiting.resolve({ username: username, password: password });
  });

  $("ap-cancel").addEventListener("click", function () {
    var waiting = approvalPending;
    closeApproval();
    if (waiting) waiting.reject(new Error("Owner approval cancelled — nothing was saved."));
  });

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

    // The Customer section stays — a locked card must still say who it was billed to —
    // but its editing controls go with everything else.
    $("customer-search").hidden = on || !!customer;
    $("customer-detach").hidden = on;
    if (on) {
      $("customer-miss").hidden = true;
      hideNewCustomer();
    }
    // A locked card still says which list it was billed from, as a plain label: the
    // dropdown would imply the saved invoice could be repriced, and it cannot.
    renderPricing();
    $("add-item").hidden = on;
    $("txn-actions").hidden = on;
    // A saved invoice's discount is part of the document now, not a control.
    $("invoice-discount").hidden = on;
    if (on) closeLineDiscount();

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
      invoice_discount_type: invoiceDiscount.type,
      invoice_discount_value: invoiceDiscount.value,
      // No rate and no tax rate. Both are core's to resolve, from the price list this
      // customer is actually on rather than from whatever this screen last displayed —
      // and the totals the screen *did* display are sent separately as `expected`, so a
      // stale price is refused rather than billed. The discounts travel as the type and
      // value they were entered as; core works out the rupees.
      lines: rows.map(function (row) {
        return {
          item_id: row.item_id,
          qty: row.qty,
          rate: null,
          tax_rate: null,
          discount_type: row.discount_type || "none",
          discount_value: row.discount_value || 0,
        };
      }),
    };
  }

  function saveError(message, fix) {
    actionMsg(message, true, fix);
    status("Not saved: " + message);
  }

  /**
   * Saves the transaction. The invoice number is allocated by core inside the same
   * transaction that writes the rows — never previewed here beforehand, so two consoles
   * billing at the same moment cannot be shown the same number.
   */
  function printAndLock() {
    if (locked) return; // already saved; New Transaction is the only way on

    if (!customer) {
      // Not just a complaint: the message takes them to the box that satisfies it, and
      // the section is scrolled to and focused straight away so a keyboard-only operator
      // is already typing the mobile number.
      focusCustomerSection();
      return saveError("Attach a customer first.", {
        label: "Go to Customer",
        run: focusCustomerSection,
      });
    }
    if (!rows.length) return saveError("Add at least one item.");
    if (!lastQuote) return saveError("Totals not priced yet — try again.");

    var bad = rows.filter(function (row) {
      return !(row.qty > 0);
    });
    if (bad.length) return saveError("Every row needs a quantity above zero.");

    if (!invoke) return bridgeMissing("create_invoice");

    var button = $("print-lock");
    button.disabled = true;
    actionMsg("Saving…", false);

    // What the panel is showing, discounts included. Core refuses the save if it prices
    // any of it differently.
    var expected = {
      subtotal: lastQuote.subtotal,
      cgst: lastQuote.cgst,
      sgst: lastQuote.sgst,
      igst: lastQuote.igst,
      grand_total: lastQuote.grand_total,
      pre_discount_subtotal: lastQuote.pre_discount_subtotal,
      discount_amount: lastQuote.discount_amount,
    };

    // Asked for here so the password reaches the same call that does the checking. The
    // prompt is a convenience: a cashier who skipped it is refused by core anyway, and
    // the same password covers both gates because an owner signing off a sale signs off
    // whatever about it needed signing.
    var percent = lastQuote.pre_discount_subtotal > 0
      ? (lastQuote.discount_amount / lastQuote.pre_discount_subtotal) * 100
      : 0;

    creditApprovalReason(lastQuote.grand_total)
      .then(function (creditReason) {
        if (needsApproval(percent)) return askOwnerApproval(discountReason(percent));
        if (creditReason) return askOwnerApproval(creditReason);
        return null;
      })
      .then(function (credentials) {
        return invoke("create_invoice", {
          payload: buildPayload(),
          expected: expected,
          approval: credentials,
        });
      })
      .then(function (saved) {
        button.disabled = false;
        showSaved(saved);
        // Billing on credit moves what they owe, and the badge is beside their name.
        loadOutstanding();
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

    // Render what was actually stored, not what was on screen a moment ago. The Total
    // column means the same thing locked as it did while editing: the line after its own
    // discount, with the invoice-level share left to the totals block.
    saved.lines.forEach(function (line, index) {
      var afterLineDiscount = gstRound(line.line_total - line.discount_amount);
      var cell = document.querySelector('#items-body [data-total="' + index + '"]');
      if (cell) cell.textContent = money(afterLineDiscount);
      if (rows[index]) {
        rows[index].total = afterLineDiscount;
        rows[index].discount_amount = line.discount_amount;
      }
    });
    renderTotals($("billing-totals"), totalsOf(invoice));

    $("saved-title").textContent = "Invoice " + invoice.invoice_no + " saved";
    $("saved-sub").textContent =
      rupees(invoice.grand_total) + " · " + saved.lines.length + " item" +
      (saved.lines.length === 1 ? "" : "s") + " · queued for sync";
    // A credit sale's locked card says what is still owed, not just how it was billed.
    $("pay-static").textContent =
      invoice.amount_due > 0
        ? invoice.payment_type.toUpperCase() + " · " + rupees(invoice.amount_due) + " DUE"
        : invoice.payment_type.toUpperCase();

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
    $("customer-miss").hidden = true;
    hideNewCustomer();
    invoiceDiscount = { type: "none", value: 0 };
    $("id-type").value = "none";
    $("id-value").value = "";
    syncInvoiceDiscountControls();
    setMsg("id-msg", "", false);
    closeLineDiscount();
    setPayment("cash");
    $("inv-no").hidden = true;
    $("inv-no").textContent = "";

    setLocked(false);
    clearMessage();

    renderCustomer();
    renderRows();
    requote();
    // The next bill starts as a walk-in, at whatever the default list says today.
    syncPriceListToCustomer();
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
              escapeHtml(row.invoice.payment_type) + " " +
              paymentStatusBadge(row.invoice) + "</td>" +
              '<td class="py-2 text-right font-mono tabular-nums">' +
              money(row.invoice.grand_total) +
              (row.invoice.amount_due > 0
                ? '<span class="block text-2xs text-warning">' +
                  money(row.invoice.amount_due) + " due</span>"
                : "") +
              "</td>" +
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
          meta(
            "Status",
            invoice.amount_due > 0
              ? statusWords(invoice.payment_status) + " · " + rupees(invoice.amount_due) +
                " due"
              : statusWords(invoice.payment_status)
          ) +
          meta("Sync", invoice.sync_status);

        // Same row renderer as the billing table, without the editable controls.
        $("d-lines").innerHTML = lineRowsHtml(detailLines(detail), false);
        renderTotals($("detail-totals"), totalsOf(invoice));

        $("d-gst-mode").textContent =
          (invoice.igst > 0 ? "Inter-state" : "Intra-state") +
          " · place of supply " + buyer.place_of_supply;

        // Owner-only. The command refuses anyone else too — hiding the button is the
        // courtesy, not the control.
        $("credit-form").hidden = true;
        $("d-credit").hidden = !canIssueCredits();
        loadCreditHistory(invoice.id);

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
        custom: entry.custom,
        qty: entry.line.qty,
        rate: entry.line.rate,
        tax_rate: entry.line.tax_rate,
        // Straight off the stored row. A reprint shows the discount the invoice was
        // billed with, not one worked out again from today's prices.
        discount_type: entry.line.discount_type,
        discount_value: entry.line.discount_value,
        discount_amount: entry.line.discount_amount,
        total: gstRound(entry.line.line_total - entry.line.discount_amount),
      };
    });
  }

  function closeDetail() {
    openDetail = null;
    creditable = [];
    $("credit-form").hidden = true;
    $("credit-history").hidden = true;
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

    // A Discount column only when there is one, so an ordinary bill keeps its layout.
    var anyDiscount = invoice.discount_amount > 0;
    var discountHead = anyDiscount ? '<th class="num">Discount</th>' : "";
    var discountRows = anyDiscount
      ? "<tr><th>Subtotal</th><td>" + rupees(invoice.pre_discount_subtotal) + "</td></tr>" +
        "<tr><th>Discount</th><td>−" + rupees(invoice.discount_amount) + "</td></tr>" +
        "<tr><th>Taxable Value</th><td>" + rupees(invoice.subtotal) + "</td></tr>"
      : "<tr><th>Subtotal</th><td>" + rupees(invoice.subtotal) + "</td></tr>";

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
        '<th class="num">Qty</th><th class="num">Rate</th>' + discountHead +
        '<th class="num">Tax %</th><th class="num">Amount</th>' +
      "</tr></thead><tbody>" +
      lines
        .map(function (line, index) {
          return (
            "<tr><td>" + (index + 1) + "</td>" +
            // A one-off's code is a generated key. The description is what the line is;
            // printing a UUID beside it would only make the sheet look broken.
            "<td>" + (line.custom ? "—" : escapeHtml(line.item_code)) + "</td>" +
            "<td>" + escapeHtml(line.description) + "</td>" +
            "<td>" + escapeHtml(line.uom) + "</td>" +
            '<td class="num">' + line.qty + "</td>" +
            '<td class="num">' + money(line.rate) + "</td>" +
            (anyDiscount
              ? '<td class="num">' +
                (line.discount_amount > 0 ? "−" + money(line.discount_amount) : "—") +
                "</td>"
              : "") +
            '<td class="num">' + money(line.tax_rate) + "</td>" +
            // The line after its own discount. The invoice-level share is in the totals
            // block rather than here, so no rupee is shown coming off twice.
            '<td class="num">' + money(line.total) + "</td></tr>"
          );
        })
        .join("") +
      "</tbody></table>" +
      '<table class="sheet-totals">' +
        discountRows +
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
        custom: !!source.custom,
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
    clearLockout();
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
    // Before anything is billed: the picker quotes from the active list, so it has to
    // exist before the first search, and the panel has to know where the discount
    // ceiling sits before it can say whether a bill is over it.
    loadPriceLists();
    loadDiscountLimit();
    refreshAbout();
    startSyncPolling();
    showPane("billing");
    $("mobile-input").focus();
  }

  /** Ticks the lockout countdown down. Cleared whenever the screen changes state. */
  var lockoutTimer = null;

  /** Which username is shut out. A lockout is per-account, and so is this. */
  var lockedUsername = null;

  function clearLockout() {
    if (lockoutTimer) clearInterval(lockoutTimer);
    lockoutTimer = null;
    lockedUsername = null;
    $("login-pass").disabled = false;
    $("login-submit").disabled = false;
  }

  /** `95` → `1:35`. Seconds matter here: a countdown that only moves once a minute
   *  looks stuck, and somebody waiting out a lockout is watching it. */
  function countdown(seconds) {
    var mins = Math.floor(seconds / 60);
    var secs = seconds % 60;
    return mins + ":" + (secs < 10 ? "0" : "") + secs;
  }

  /**
   * The locked-out state. Deliberately not the generic error line: the account is shut,
   * the password on screen may well be right, and saying "incorrect" would send somebody
   * hunting for a mistake they have not made.
   */
  function showLockout(username, seconds) {
    clearLockout();
    lockedUsername = username;
    // The username box stays usable on purpose. One account being shut out must not stop
    // a colleague signing in at the same till — the lockout is on a name, not on the
    // machine, and disabling the whole form would turn one person's typo into everybody's
    // problem.
    $("login-pass").disabled = true;
    $("login-submit").disabled = true;
    $("login-pass").value = "";

    var left = Math.max(0, Math.ceil(seconds));

    function paint() {
      if (left <= 0) {
        clearLockout();
        setMsg("login-msg", "You can try again now.", false);
        $("login-user").focus();
        return;
      }
      setMsg(
        "login-msg",
        "Too many failed attempts. Try again in " + countdown(left) + ".",
        true
      );
      left -= 1;
    }

    paint();
    lockoutTimer = setInterval(paint, 1000);
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
        clearLockout();
        user = session.user;
        setMsg("login-msg", "", false);
        newTransaction();
        showShell();
        status("Signed in as " + user.display_name + ".");
      })
      .catch(function (err) {
        // The backend answers with a shape, not just a string: a lockout has to look
        // different from a wrong password, and it carries how long is left.
        if (err && err.kind === "locked_out") {
          showLockout(username.toLowerCase(), err.retry_after_seconds || 0);
          return;
        }

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

  // Typing a different username lifts the display: that account is not the locked one.
  // The limiter in core is still the thing that decides — this only stops the screen
  // showing one account's lockout while somebody types another's name.
  $("login-user").addEventListener("input", function () {
    if (lockedUsername && $("login-user").value.trim().toLowerCase() !== lockedUsername) {
      clearLockout();
      setMsg("login-msg", "", false);
    }
  });

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
    UI.enhancePasswordFields($("user-form"));
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

    // Cashiers do not see either of these; the commands refuse them anyway.
    $("demo-data").hidden = user.role !== "owner";
    $("price-lists-section").hidden = user.role !== "owner";
    $("discount-limit-section").hidden = user.role !== "owner";
    setMsg("demo-msg", "", false);
    setMsg("pl-msg", "", false);
    setMsg("dl-msg", "", false);
    $("dl-value").value = discountLimitPct;
    renderPriceListRows();

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

  /* ---------------------------------------------------------------- customers */

  /** The rows the Customers list is showing, and whose ledger is open. */
  var customerRows = [];
  var openLedger = null;

  function loadCustomers() {
    if (!invoke || !user) return;

    invoke("list_customers", {
      filter: {
        text: $("cu-filter").value.trim() || null,
        owing_only: $("cu-owing").checked,
        sort: $("cu-sort").value,
      },
    })
      .then(function (rows) {
        customerRows = rows;
        $("customers-body").innerHTML = rows
          .map(function (row, index) {
            var owed = row.outstanding;
            return (
              '<tr class="cursor-pointer border-b border-base-300 text-sm last:border-0 ' +
              'hover:bg-base-200/60" data-customer="' + index + '">' +
              '<td class="py-2 font-medium">' + escapeHtml(row.customer.name) + "</td>" +
              '<td class="py-2 font-mono text-base-content/60">' +
              escapeHtml(row.customer.mobile) + "</td>" +
              '<td class="py-2 pr-4 text-right font-mono tabular-nums text-base-content/60">' +
              row.invoice_count + "</td>" +
              '<td class="py-2 font-mono text-xs text-base-content/45">' +
              escapeHtml(row.last_billed || "—") + "</td>" +
              '<td class="py-2 text-right font-mono tabular-nums ' +
              // Only money actually owed is worth colouring. Zero is the normal case and
              // a screen of red zeroes says nothing.
              (owed > 0 ? "font-medium text-warning" : "text-base-content/45") + '">' +
              (owed < 0 ? "−" + money(-owed) : money(owed)) +
              "</td></tr>"
            );
          })
          .join("");

        $("customers-empty").hidden = rows.length > 0;
        var owing = rows.filter(function (r) { return r.outstanding > 0; });
        var total = owing.reduce(function (sum, r) { return sum + r.outstanding; }, 0);
        $("customers-count").textContent =
          rows.length + " customer(s)" +
          (owing.length
            ? " · " + owing.length + " owing " + rupees(total)
            : " · nobody owes anything");
      })
      .catch(function (err) {
        status("list_customers failed: " + errText(err));
      });
  }

  $("cu-filter").addEventListener("input", loadCustomers);
  $("cu-sort").addEventListener("change", loadCustomers);
  $("cu-owing").addEventListener("change", loadCustomers);

  $("customers-body").addEventListener("click", function (event) {
    var row = event.target.closest("[data-customer]");
    if (!row) return;
    var picked = customerRows[Number(row.dataset.customer)];
    if (picked) showLedger(picked.customer.id);
  });

  /* ------------------------------------------------------------------ ledger */

  function showLedger(customerId) {
    if (!invoke) return bridgeMissing("customer_ledger");

    invoke("customer_ledger", { customerId: customerId })
      .then(function (ledger) {
        openLedger = ledger;
        renderLedger(ledger);
        $("customers-list-card").hidden = true;
        $("ledger-card").hidden = false;
        closePaymentForm();
        status(ledger.customer.name + ": " + describeBalance(ledger.balance) + ".");
      })
      .catch(function (err) {
        status("customer_ledger failed: " + errText(err));
      });
  }

  function closeLedger() {
    openLedger = null;
    $("ledger-card").hidden = true;
    $("customers-list-card").hidden = false;
    loadCustomers();
  }

  /** "owes ₹5,248.00", "in credit by ₹200.00", or "settled up". */
  function describeBalance(balance) {
    if (balance > 0) return "owes " + rupees(balance);
    if (balance < 0) return "in credit by " + rupees(-balance);
    return "settled up";
  }

  function renderLedger(ledger) {
    var buyer = ledger.customer;
    $("lg-name").textContent = buyer.name;
    $("lg-meta").textContent =
      buyer.mobile + " · GSTIN " + (buyer.gstin || "unregistered") +
      " · place of supply " + buyer.place_of_supply +
      (buyer.credit_limit != null ? " · credit limit " + rupees(buyer.credit_limit) : "");

    // Owing, in credit and settled are three different facts, so they get three
    // different labels rather than one number that changes sign.
    $("lg-balance-label").textContent =
      ledger.balance < 0 ? "In credit" : "Outstanding";
    $("lg-balance").textContent =
      ledger.balance < 0 ? rupees(-ledger.balance) : rupees(ledger.balance);
    $("lg-balance").className =
      "font-mono text-3xl font-semibold tabular-nums " +
      (ledger.balance > 0 ? "text-warning" : ledger.balance < 0 ? "text-success" : "");

    $("lg-billed").textContent = rupees(ledger.billed_total);
    $("lg-paid").textContent = rupees(ledger.paid_total);
    $("lg-credited").textContent = rupees(ledger.credited_total);

    $("lg-limit-row").hidden = !user || user.role !== "owner";
    $("lg-limit").value = buyer.credit_limit == null ? "" : buyer.credit_limit;
    setMsg("lg-limit-msg", "", false);

    var tones = {
      invoice: "bg-base-300 text-base-content/70",
      payment: "bg-success/15 text-success",
      credit_note: "bg-info/15 text-info",
    };
    var labels = { invoice: "Invoice", payment: "Payment", credit_note: "Credit note" };

    $("ledger-body").innerHTML = ledger.entries
      .map(function (entry) {
        return (
          '<tr class="border-b border-base-300 text-sm last:border-0">' +
          '<td class="py-2 font-mono text-xs text-base-content/60">' +
          escapeHtml(entry.date) + "</td>" +
          '<td class="py-2"><span class="rounded-field px-1.5 py-0.5 text-2xs ' +
          'font-medium uppercase tracking-wider ' + (tones[entry.kind] || tones.invoice) +
          '">' + escapeHtml(labels[entry.kind] || entry.kind) + "</span>" +
          '<span class="ml-2 font-mono text-xs">' + escapeHtml(entry.reference) +
          "</span></td>" +
          '<td class="py-2 text-base-content/60">' + escapeHtml(entry.description) + "</td>" +
          '<td class="py-2 text-right font-mono tabular-nums ' +
          (entry.change < 0 ? "text-success" : "") + '">' +
          (entry.change < 0 ? "−" + money(-entry.change) : money(entry.change)) + "</td>" +
          '<td class="py-2 text-right font-mono tabular-nums font-medium">' +
          (entry.balance < 0 ? "−" + money(-entry.balance) : money(entry.balance)) +
          "</td></tr>"
        );
      })
      .join("");
    $("ledger-empty").hidden = ledger.entries.length > 0;
  }

  $("lg-back").addEventListener("click", closeLedger);

  $("lg-limit-save").addEventListener("click", function () {
    if (!invoke || !openLedger) return;
    var raw = $("lg-limit").value.trim();
    var limit = raw === "" ? null : parseFloat(raw);
    if (limit !== null && (!isFinite(limit) || limit < 0)) {
      return setMsg("lg-limit-msg", "Enter a limit of zero or more, or leave it blank.", true);
    }

    setMsg("lg-limit-msg", "Saving…", false);
    invoke("set_credit_limit", {
      customerId: openLedger.customer.id,
      creditLimit: limit,
    })
      .then(function () {
        setMsg(
          "lg-limit-msg",
          limit === null ? "Limit removed." : "Limit set to " + rupees(limit) + ".",
          false
        );
        return showLedger(openLedger.customer.id);
      })
      .catch(function (err) {
        setMsg("lg-limit-msg", errText(err), true);
      });
  });

  /* --------------------------------------------------------- record payment */

  function openPaymentForm() {
    if (!openLedger) return;

    $("pm-amount").value = "";
    $("pm-method").value = "cash";
    $("pm-date").value = todayIso();
    $("pm-notes").value = "";
    setMsg("pm-msg", "", false);

    // Open bills, oldest first, plus the "no particular bill" option a customer
    // clearing a running balance needs.
    $("pm-invoice").innerHTML =
      '<option value="">The account (no particular bill)</option>' +
      openLedger.open_invoices
        .map(function (invoice) {
          return (
            '<option value="' + invoice.id + '">' +
            escapeHtml(invoice.invoice_no) + " · " + rupees(invoice.amount_due) + " due" +
            "</option>"
          );
        })
        .join("");
    // The oldest open bill is what a shop settles first, so it is preselected.
    if (openLedger.open_invoices.length) {
      $("pm-invoice").value = String(openLedger.open_invoices[0].id);
    }

    $("payment-form").hidden = false;
    $("pm-amount").focus();
  }

  function closePaymentForm() {
    $("payment-form").hidden = true;
  }

  function todayIso() {
    var now = new Date();
    return (
      now.getFullYear() + "-" +
      String(now.getMonth() + 1).padStart(2, "0") + "-" +
      String(now.getDate()).padStart(2, "0")
    );
  }

  function submitPayment(event) {
    if (event) event.preventDefault();
    if (!invoke || !openLedger) return bridgeMissing("record_payment");

    var amount = parseFloat($("pm-amount").value);
    if (!isFinite(amount) || amount <= 0) {
      return setMsg("pm-msg", "Enter an amount above zero.", true);
    }

    var invoiceId = $("pm-invoice").value ? Number($("pm-invoice").value) : null;
    var button = $("pm-save");
    button.disabled = true;
    setMsg("pm-msg", "Recording…", false);

    invoke("record_payment", {
      payment: {
        customer_id: openLedger.customer.id,
        invoice_id: invoiceId,
        amount: amount,
        payment_method: $("pm-method").value,
        date: $("pm-date").value || null,
        notes: $("pm-notes").value.trim() || null,
      },
    })
      .then(function (recorded) {
        button.disabled = false;
        closePaymentForm();

        // A payment bigger than the bill it was aimed at becomes two rows. Saying so is
        // the difference between a customer trusting the receipt and wondering where the
        // rest of their money went.
        if (recorded.applied && recorded.on_account) {
          UI.toast(
            rupees(recorded.applied.amount) + " settled the invoice; " +
              rupees(recorded.on_account.amount) + " went to the account.",
            "success"
          );
        } else {
          UI.toast("Recorded " + rupees(amount) + ".", "success");
        }
        status(
          openLedger.customer.name + " now " + describeBalance(recorded.balance) + "."
        );
        return showLedger(openLedger.customer.id);
      })
      .catch(function (err) {
        button.disabled = false;
        setMsg("pm-msg", errText(err), true);
      });
  }

  $("lg-pay").addEventListener("click", openPaymentForm);
  $("pm-cancel").addEventListener("click", closePaymentForm);
  $("payment-form").addEventListener("submit", submitPayment);

  /* ------------------------------------------------ price list management */

  /** The Settings table. Owner-only; the commands behind it refuse anyone else. */
  function renderPriceListRows() {
    var body = $("pl-rows");
    if (!body) return;

    body.innerHTML = priceLists
      .map(function (list, index) {
        return (
          '<tr class="border-b border-base-300 text-sm last:border-0">' +
          '<td class="py-2 font-medium">' + escapeHtml(list.name) + "</td>" +
          '<td class="py-2">' +
          (list.is_default
            ? '<span class="rounded-field bg-base-300 px-2 py-0.5 text-2xs font-medium ' +
              'uppercase tracking-wider text-base-content/70">default</span>'
            : '<button type="button" data-default="' + index +
              '" class="text-xs text-base-content/60 underline-offset-2 hover:underline ' +
              'hover:text-base-content">Make default</button>') +
          "</td>" +
          '<td class="py-2 text-right"><button type="button" data-rename="' + index +
          '" class="text-xs text-base-content/60 underline-offset-2 hover:underline ' +
          'hover:text-base-content">Rename</button></td>' +
          "</tr>"
        );
      })
      .join("");
  }

  function addPriceList() {
    if (!invoke) return bridgeMissing("create_price_list");
    var name = $("pl-name").value.trim();
    if (!name) return setMsg("pl-msg", "Name the list first.", true);

    setMsg("pl-msg", "Creating…", false);
    invoke("create_price_list", { name: name })
      .then(function (created) {
        $("pl-name").value = "";
        setMsg("pl-msg", "Created " + created.name + ".", false);
        return reloadPriceLists();
      })
      .catch(function (err) {
        setMsg("pl-msg", errText(err), true);
      });
  }

  /**
   * Reloads the lists and re-resolves the bill against them.
   *
   * Moving the default changes what a walk-in pays, so a half-built bill on the Billing
   * pane has to follow rather than sit on rates that no longer exist anywhere.
   */
  function reloadPriceLists() {
    return loadPriceLists().then(function () {
      return syncPriceListToCustomer();
    });
  }

  $("pl-add").addEventListener("click", addPriceList);
  $("pl-name").addEventListener("keydown", function (event) {
    if (event.key === "Enter") addPriceList();
  });

  $("pl-rows").addEventListener("click", function (event) {
    var makeDefault = event.target.closest("[data-default]");
    var rename = event.target.closest("[data-rename]");
    if (!invoke) return;

    if (makeDefault) {
      var chosen = priceLists[Number(makeDefault.dataset.default)];
      if (!chosen) return;
      setMsg("pl-msg", "Updating…", false);
      invoke("set_default_price_list", { id: chosen.id })
        .then(function (list) {
          setMsg("pl-msg", list.name + " is now the default.", false);
          return reloadPriceLists();
        })
        .catch(function (err) {
          setMsg("pl-msg", errText(err), true);
        });
      return;
    }

    if (rename) {
      var target = priceLists[Number(rename.dataset.rename)];
      if (!target) return;
      var next = window.prompt("Rename this price list:", target.name);
      if (next == null || !next.trim() || next.trim() === target.name) return;
      invoke("rename_price_list", { id: target.id, name: next.trim() })
        .then(function (list) {
          setMsg("pl-msg", "Renamed to " + list.name + ".", false);
          return reloadPriceLists();
        })
        .catch(function (err) {
          setMsg("pl-msg", errText(err), true);
        });
    }
  });

  /** The dropdown on the customer chip. Owner-only, and the command enforces that. */
  $("pricing-select").addEventListener("change", function (event) {
    if (!invoke || !customer) return;
    var value = event.target.value;
    var priceListId = value ? Number(value) : null;

    invoke("set_customer_price_list", {
      customerId: customer.id,
      priceListId: priceListId,
    })
      .then(function (updated) {
        customer = updated;
        renderCustomer();
        return syncPriceListToCustomer();
      })
      .then(function () {
        status(customer.name + " is billed from " + (activeList ? activeList.name : "—") + ".");
      })
      .catch(function (err) {
        // Put the control back to what the database still says.
        renderPricing();
        status(errText(err));
      });
  });

  /* -------------------------------------------------------------- demo data */

  /**
   * Clears the sample catalogue.
   *
   * The wipe itself is core's, and it is deliberately narrow: a seeded item or customer
   * goes only if nothing has ever been billed against it. An invoice is a document that
   * has left the building, so anything it points at has to stay readable for as long as
   * the invoice does — which is why this reports what it kept as well as what it removed.
   */
  function clearDemoData() {
    if (!invoke) return bridgeMissing("clear_demo_data");

    // A bill in progress holds item ids that this is about to delete, and the save would
    // fail on a foreign key with nothing useful to say. Finish or clear the bill first.
    if (rows.length && !locked) {
      return setMsg(
        "demo-msg",
        "Finish or clear the transaction on the Billing screen first.",
        true
      );
    }

    if (!window.confirm(
      "Remove the seeded sample customers and stock items?\n\n" +
      "Anything already billed is kept, along with every invoice and credit note. " +
      "This cannot be undone."
    )) {
      return;
    }

    var button = $("demo-clear");
    button.disabled = true;
    setMsg("demo-msg", "Clearing…", false);

    invoke("clear_demo_data")
      .then(function (result) {
        button.disabled = false;
        var kept = result.items_kept + result.customers_kept;
        setMsg(
          "demo-msg",
          "Removed " + result.items_removed + " item(s) and " +
            result.customers_removed + " customer(s)." +
            (kept ? " Kept " + kept + " that have been billed." : ""),
          false
        );
        status("Demo data cleared.");
      })
      .catch(function (err) {
        button.disabled = false;
        setMsg("demo-msg", errText(err), true);
      });
  }

  $("demo-clear").addEventListener("click", clearDemoData);

  /**
   * Loads the shop's discount ceiling. Read by everyone — a cashier needs to know where
   * the line is before they hit it — but only an owner can move it, and only core can
   * enforce it.
   */
  function loadDiscountLimit() {
    if (!invoke) return Promise.resolve();
    return invoke("discount_approval_threshold")
      .then(function (percent) {
        discountLimitPct = percent;
        $("dl-value").value = percent;
      })
      .catch(function (err) {
        status("discount_approval_threshold failed: " + errText(err));
      });
  }

  $("dl-save").addEventListener("click", function () {
    if (!invoke) return bridgeMissing("set_discount_approval_threshold");
    var percent = parseFloat($("dl-value").value);
    if (!isFinite(percent) || percent < 0 || percent > 100) {
      return setMsg("dl-msg", "Enter a percentage between 0 and 100.", true);
    }

    setMsg("dl-msg", "Saving…", false);
    invoke("set_discount_approval_threshold", { percent: percent })
      .then(function (saved) {
        discountLimitPct = saved;
        $("dl-value").value = saved;
        setMsg("dl-msg", "Cashiers may now discount up to " + money(saved) + "%.", false);
        requote();
      })
      .catch(function (err) {
        setMsg("dl-msg", errText(err), true);
      });
  });

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
    $("it-tax").value = existing ? String(existing.tax_rate) : "18";
    $("it-uom").value = existing ? existing.uom : "";
    // The code is the key core matches on, so changing it while editing would quietly
    // create a second item rather than rename this one.
    $("it-code").readOnly = !!existing;
    $("it-code").classList.toggle("bg-base-200", !!existing);
    setMsg("it-msg", "", false);

    // Drawn empty first so the form is never showing the previous item's prices while
    // this one's are still in flight.
    renderItemPrices(existing ? existing.rate : null, []);
    if (existing && invoke) {
      invoke("item_prices", { itemId: existing.id })
        .then(function (rowsForItem) {
          renderItemPrices(existing.rate, rowsForItem);
        })
        .catch(function (err) {
          setMsg("it-msg", errText(err), true);
        });
    }

    (existing ? $("it-desc") : $("it-code")).focus();
  }

  /**
   * The item's prices: the base rate, then one row per list.
   *
   * The base rate is required and is the fallback; a list row left blank has no entry at
   * all, which is what makes it fall back rather than bill zero. That distinction is why
   * the boxes are empty rather than pre-filled with the base rate — a number in the box
   * would read as "set", and saving would pin the list to today's base rate forever.
   */
  function renderItemPrices(baseRate, priceRows) {
    var box =
      "h-8 w-full max-w-36 rounded-field border border-base-300 bg-base-100 px-2 " +
      "text-right font-mono text-sm tabular-nums focus:border-base-content/30 " +
      "focus:outline-none focus:ring-2 focus:ring-base-content/10";

    var base =
      '<tr class="border-b border-base-300 text-sm">' +
      '<td class="py-2"><span class="font-medium">Base rate</span>' +
      '<span class="block text-xs text-base-content/45">Billed by any list with no rate ' +
      "of its own</span></td>" +
      '<td class="py-2 text-right"><input id="it-rate" type="number" min="0" step="0.01" ' +
      'required aria-label="Base rate" value="' +
      (baseRate == null ? "" : baseRate) + '" class="' + box + ' ml-auto" /></td>' +
      "</tr>";

    var lists = priceLists
      .map(function (list) {
        var match = (priceRows || []).filter(function (r) {
          return r.price_list.id === list.id;
        })[0];
        var value = match && match.rate != null ? match.rate : "";
        return (
          '<tr class="border-b border-base-300 text-sm last:border-0">' +
          '<td class="py-2">' + escapeHtml(list.name) +
          (list.is_default
            ? ' <span class="text-xs text-base-content/45">· default</span>'
            : "") +
          "</td>" +
          '<td class="py-2 text-right"><input type="number" min="0" step="0.01" ' +
          'data-price-list="' + list.id + '" placeholder="base" aria-label="' +
          escapeHtml(list.name) + ' rate" value="' + value + '" class="' + box +
          ' ml-auto" /></td>' +
          "</tr>"
        );
      })
      .join("");

    $("it-prices").innerHTML = base + lists;
  }

  /** The list rows of the price table, as core wants them. Blank rows are simply absent. */
  function itemPricePayload(itemId) {
    var out = [];
    Array.prototype.forEach.call(
      $("it-prices").querySelectorAll("input[data-price-list]"),
      function (input) {
        var text = input.value.trim();
        if (!text) return;
        var rate = parseFloat(text);
        if (!isFinite(rate) || rate < 0) return;
        out.push({
          item_id: itemId,
          price_list_id: Number(input.dataset.priceList),
          rate: rate,
        });
      }
    );
    return out;
  }

  function saveItem(event) {
    if (event) event.preventDefault();
    if (!invoke) return bridgeMissing("save_item");

    var code = $("it-code").value.trim();
    var description = $("it-desc").value.trim();
    var rate = parseFloat($("it-rate").value);

    if (!code) return setMsg("it-msg", "Enter an item code.", true);
    if (!description) return setMsg("it-msg", "Enter a description.", true);
    if (!isFinite(rate) || rate < 0) {
      return setMsg("it-msg", "Enter a base rate of 0 or more.", true);
    }

    var badList = null;
    Array.prototype.forEach.call(
      $("it-prices").querySelectorAll("input[data-price-list]"),
      function (input) {
        var text = input.value.trim();
        if (!text) return;
        var listRate = parseFloat(text);
        if (!isFinite(listRate) || listRate < 0) {
          badList = input.getAttribute("aria-label") || "A price list";
        }
      }
    );
    if (badList) return setMsg("it-msg", badList + " must be 0 or more, or blank.", true);

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
        // The prices go in a second call because a new item has no id until the first
        // one returns. Core replaces the whole table for this item, so a list the form
        // left blank has its entry removed rather than quietly kept.
        return invoke("set_item_prices", {
          itemId: saved.id,
          prices: itemPricePayload(saved.id),
        }).then(function () {
          return saved;
        });
      })
      .then(function (saved) {
        button.disabled = false;
        showItemForm(false);
        loadCatalogue();
        status("Saved " + saved.item_code + " and its prices.");
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

        // Net leads, because that is what was earned and what a return is filed on.
        // Billed stays beside it: both are true, and a pane that showed only one would be
        // answering a different question from the one asked.
        $("an-stats").innerHTML =
          UI.statCard("hero-document-text", "Invoices", String(s.invoice_count),
                      s.credit_note_count > 0
                        ? s.credit_note_count + " credit note(s)"
                        : null) +
          UI.statCard("hero-banknotes", "Net earned", rupees(s.net_total),
                      s.credited_total > 0
                        ? rupees(s.grand_total) + " billed less " +
                          rupees(s.credited_total) + " credited"
                        : "including tax") +
          UI.statCard("hero-credit-card", "Taxable value", rupees(s.net_subtotal)) +
          UI.statCard("hero-check-circle", "Tax collected", rupees(s.net_tax),
                      "CGST + SGST + IGST, net of credits");

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

        // Slices are already net of credits. A payment type that has been fully refunded
        // contributes nothing, and a donut cannot draw a negative slice, so anything at or
        // below zero is left out rather than drawn wrong.
        $("an-payments").innerHTML = UI.donutChart(
          report.payments
            .filter(function (p) {
              return p.grand_total > 0;
            })
            .map(function (p) {
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
              '<td class="py-2 text-right font-mono tabular-nums">' + money(row.qty) +
              (row.credited_qty > 0
                ? '<span class="ml-1 text-2xs text-base-content/45">(−' +
                  money(row.credited_qty) + ")</span>"
                : "") +
              "</td>" +
              '<td class="py-2 text-right font-mono tabular-nums">' + money(row.revenue) +
              "</td>" +
              "</tr>"
            );
          })
          .join("");

        status(
          s.invoice_count + " invoice(s) · " + rupees(s.grand_total) + " billed" +
            (s.credited_total > 0
              ? " · " + rupees(s.credited_total) + " credited · " +
                rupees(s.net_total) + " net"
              : "") +
            "."
        );
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


  /* ----------------------------------------------------------------- credit notes */

  /** What is still creditable on the open invoice, as last loaded. */
  var creditable = [];

  /** Only an owner sees the action at all — and the command refuses anyone else. */
  function canIssueCredits() {
    return !!user && user.role === "owner";
  }

  /**
   * The credit notes already against the open invoice, and what they net it to.
   * Shown to everyone signed in: a cashier looking at a partly-returned invoice needs to
   * know, or the figure on their screen is wrong.
   */
  function loadCreditHistory(invoiceId) {
    if (!invoke) return;

    invoke("credit_history", { invoiceId: invoiceId })
      .then(function (history) {
        var notes = history.notes || [];
        $("credit-history").hidden = notes.length === 0;
        if (!notes.length) return;

        $("credit-list").innerHTML = notes
          .map(function (entry) {
            var note = entry.credit_note;
            var lines = entry.lines
              .map(function (line) {
                var source = lineLabel(line.item_id);
                return (
                  '<tr class="border-b border-base-300 text-sm last:border-0">' +
                  '<td class="py-2 font-mono">' + escapeHtml(source.code) + "</td>" +
                  '<td class="py-2">' + escapeHtml(source.description) + "</td>" +
                  '<td class="py-2 text-right font-mono tabular-nums">' + money(line.qty) +
                  "</td>" +
                  '<td class="py-2 text-right font-mono tabular-nums">' + money(line.rate) +
                  "</td>" +
                  '<td class="py-2 text-right font-mono tabular-nums">' +
                  money(line.line_total) + "</td>" +
                  "</tr>"
                );
              })
              .join("");

            return (
              '<div class="mb-4 rounded-field border border-base-300 p-4 last:mb-0">' +
              '<div class="flex flex-wrap items-baseline justify-between gap-3">' +
              '<span class="font-mono text-sm font-medium">' +
              escapeHtml(note.credit_note_no) + "</span>" +
              '<span class="font-mono text-sm tabular-nums">−' + rupees(note.grand_total) +
              "</span>" +
              "</div>" +
              '<p class="mt-1 text-sm text-base-content/60">' + escapeHtml(note.reason) +
              "</p>" +
              '<p class="mt-1 font-mono text-xs text-base-content/45">' +
              escapeHtml(dateOnly(note.date)) +
              (entry.created_by
                ? " · issued by " + escapeHtml(entry.created_by.display_name)
                : "") +
              "</p>" +
              '<table class="mt-3 w-full text-left"><tbody>' + lines + "</tbody></table>" +
              "</div>"
            );
          })
          .join("");

        // The figure that actually matters once corrections exist.
        var net = history.net;
        $("credit-net").innerHTML =
          netRow("Invoice total", rupees(openDetail.invoice.grand_total), false) +
          netRow("Credited", "−" + rupees(net.credited_total), false) +
          netRow("Net after credits", rupees(net.net_total), true);
      })
      .catch(function (err) {
        status("credit_history failed: " + errText(err));
      });
  }

  function netRow(label, value, strong) {
    return (
      '<div class="flex items-baseline justify-between gap-4 ' +
      (strong
        ? 'mt-2 border-t border-base-300 pt-3"><dt class="text-base font-semibold">'
        : 'py-1"><dt class="text-sm text-base-content/60">') +
      escapeHtml(label) +
      "</dt><dd class=" +
      (strong
        ? '"font-mono text-2xl font-semibold tabular-nums">'
        : '"font-mono text-sm tabular-nums">') +
      escapeHtml(value) +
      "</dd></div>"
    );
  }

  /** Item code and description for a credited line, from the invoice on screen. */
  function lineLabel(itemId) {
    var entries = openDetail && openDetail.lines ? openDetail.lines : [];
    for (var i = 0; i < entries.length; i += 1) {
      if (entries[i].line && entries[i].line.item_id === itemId) {
        return { code: entries[i].item_code, description: entries[i].description };
      }
    }
    return { code: "#" + itemId, description: "" };
  }

  /** Draws the form from what is still creditable. */
  function openCreditForm() {
    if (!invoke || !openDetail) return;

    invoke("creditable_lines", { invoiceId: openDetail.invoice.id })
      .then(function (lines) {
        creditable = lines;

        var anyLeft = lines.some(function (l) {
          return l.creditable_qty > 0;
        });
        if (!anyLeft) {
          status("Every line on this invoice has already been credited in full.");
          return;
        }

        $("credit-lines").innerHTML = lines
          .map(function (line, index) {
            var spent = line.creditable_qty <= 0;
            return (
              '<tr class="border-b border-base-300 text-sm last:border-0">' +
              '<td class="py-2 font-mono">' + escapeHtml(line.item_code) + "</td>" +
              '<td class="py-2">' + escapeHtml(line.description) + "</td>" +
              '<td class="py-2 text-right font-mono tabular-nums">' + money(line.billed_qty) +
              "</td>" +
              '<td class="py-2 pr-4 text-right font-mono tabular-nums ' +
              'text-base-content/60">' + money(line.credited_qty) + "</td>" +
              '<td class="py-2 text-right">' +
              '<input type="number" min="0" step="any" value="0" data-credit="' + index +
              '" max="' + line.creditable_qty + '"' + (spent ? " disabled" : "") +
              ' aria-label="Quantity to credit for ' + escapeHtml(line.item_code) + '"' +
              ' class="h-7 w-full max-w-24 rounded-field border border-base-300 ' +
              'bg-base-100 px-2 text-right font-mono text-sm tabular-nums ' +
              'focus:border-base-content/30 focus:outline-none focus:ring-2 ' +
              'focus:ring-base-content/10 disabled:bg-base-200 ' +
              'disabled:text-base-content/60" />' +
              "</td></tr>"
            );
          })
          .join("");

        $("credit-reason").value = "";
        setMsg("credit-msg", "", false);
        $("credit-form").hidden = false;
        $("d-credit").hidden = true;
        $("credit-reason").focus();
      })
      .catch(function (err) {
        status(errText(err));
      });
  }

  function closeCreditForm() {
    $("credit-form").hidden = true;
    $("d-credit").hidden = !canIssueCredits();
  }

  function submitCreditNote(event) {
    if (event) event.preventDefault();
    if (!invoke || !openDetail) return bridgeMissing("create_credit_note");

    var reason = $("credit-reason").value.trim();
    if (!reason) return setMsg("credit-msg", "Enter a reason for the credit.", true);

    var lines = [];
    var bad = null;
    Array.prototype.forEach.call(
      $("credit-lines").querySelectorAll("input[data-credit]"),
      function (input) {
        var source = creditable[Number(input.dataset.credit)];
        var qty = parseFloat(input.value);
        if (!isFinite(qty) || qty <= 0) return;
        if (qty > source.creditable_qty) {
          bad =
            "Cannot credit " + money(qty) + " of " + source.item_code + " — only " +
            money(source.creditable_qty) + " remain.";
          return;
        }
        lines.push({ invoice_line_id: source.invoice_line_id, qty: qty });
      }
    );

    if (bad) return setMsg("credit-msg", bad, true);
    if (!lines.length) {
      return setMsg("credit-msg", "Enter a quantity to credit on at least one line.", true);
    }

    var button = $("credit-save");
    button.disabled = true;
    setMsg("credit-msg", "Issuing…", false);

    invoke("create_credit_note", {
      note: {
        original_invoice_id: openDetail.invoice.id,
        reason: reason,
        lines: lines,
      },
    })
      .then(function (note) {
        button.disabled = false;
        closeCreditForm();
        loadCreditHistory(openDetail.invoice.id);
        // The invoice itself has not changed, so only the credits need redrawing.
        status("Issued " + note.credit_note_no + " for " + rupees(note.grand_total) + ".");
      })
      .catch(function (err) {
        button.disabled = false;
        setMsg("credit-msg", errText(err), true);
      });
  }

  $("d-credit").addEventListener("click", openCreditForm);
  $("credit-cancel").addEventListener("click", closeCreditForm);
  $("credit-form").addEventListener("submit", submitCreditNote);

  /* ----------------------------------------------------------------- wiring */

  $("search-btn").addEventListener("click", searchCustomer);
  $("mobile-input").addEventListener("keydown", function (event) {
    if (event.key === "Enter") searchCustomer();
  });

  $("customer-detach").addEventListener("click", detachCustomer);
  $("nc-open").addEventListener("click", function () {
    showNewCustomer($("mobile-input").value.trim());
  });
  $("nc-save").addEventListener("click", createCustomer);
  $("nc-cancel").addEventListener("click", function () {
    hideNewCustomer();
    // Back to the search, not to an empty section: they still have a sale to bill.
    renderCustomer();
    $("mobile-input").focus();
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
  // Every password field in the app gets a show/hide eye from here — the login screen,
  // first-run setup and Add User alike. A future one gets it by existing.
  UI.enhancePasswordFields();
  bootstrap();
})();
