// Capstan landing page: copy-to-clipboard, theme switch, and one dashboard update.
(function () {
  "use strict";

  var root = document.documentElement;
  var reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  // Copy the install command.
  var btn = document.getElementById("copy-btn");
  var status = document.getElementById("install-status");
  var text = document.getElementById("install-text");
  var resetTimer;

  function setState(state, message) {
    btn.dataset.state = state;
    status.dataset.state = state;
    btn.textContent = state === "copied" ? "Copied" : state === "error" ? "Copy again" : "Copy";
    status.textContent = message || "";
    clearTimeout(resetTimer);
    if (state === "copied") resetTimer = setTimeout(function () { setState("idle"); }, 4000);
  }

  function selectCommand() {
    var range = document.createRange();
    range.selectNodeContents(text);
    var sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
  }

  function legacyCopy() {
    selectCommand();
    try { return document.execCommand("copy"); } catch { return false; }
  }

  function failed() {
    selectCommand();
    var mac = /Mac|iPhone|iPad/.test(navigator.platform || "");
    setState("error", "The browser blocked the clipboard. The command is selected: press " + (mac ? "Cmd+C" : "Ctrl+C") + " to copy it.");
  }

  function copied() {
    setState("copied", "Copied. Paste it into a terminal to install cstan.");
  }

  btn.addEventListener("click", function () {
    var cmd = text.textContent;
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(cmd).then(copied, function () {
        if (legacyCopy()) copied();
        else failed();
      });
    } else if (legacyCopy()) {
      copied();
    } else {
      failed();
    }
  });

  // Theme switch: dark first, light on request or when the system asks for it.
  var toggle = document.getElementById("theme-toggle");
  var prefersLight = window.matchMedia("(prefers-color-scheme: light)");

  function current() { return root.dataset.theme || (prefersLight.matches ? "light" : "dark"); }
  function label() {
    var next = current() === "dark" ? "light" : "dark";
    toggle.textContent = next === "light" ? "Light" : "Dark";
    toggle.setAttribute("aria-label", "Switch to " + next + " theme");
  }
  toggle.hidden = false;
  label();
  prefersLight.addEventListener("change", label);
  toggle.addEventListener("click", function () {
    var next = current() === "dark" ? "light" : "dark";
    root.dataset.theme = next;
    try { localStorage.setItem("capstan-theme", next); } catch {}
    label();
  });

  // The dashboard shows one report moving to review. The markup holds the end state,
  // so without script or with reduced motion the table is complete and still.
  if (reduceMotion) return;
  var dash = document.getElementById("dash");
  var verified = document.getElementById("dash-verified");
  if (!dash || !verified) return;
  var dev = dash.querySelector('[data-agent="developer-2"]').cells;
  var rev = dash.querySelector('[data-agent="reviewer-3"]').cells;
  var end = [dev[2].innerHTML, dev[3].innerHTML, rev[2].innerHTML, rev[3].innerHTML, verified.textContent];

  function put(cell, html) {
    cell.innerHTML = html;
    cell.classList.remove("cap-changed");
    void cell.offsetWidth;
    cell.classList.add("cap-changed");
  }

  dev[2].innerHTML = '<span class="cap-state is-active">working</span>';
  dev[3].innerHTML = "capstan/developer-2-g1";
  rev[2].innerHTML = '<span class="cap-state">queued</span>';
  rev[3].innerHTML = "waits for a verified report";
  verified.textContent = "reports verified 3";

  setTimeout(function () {
    put(dev[2], end[0]);
    put(dev[3], end[1]);
    verified.textContent = end[4];
  }, 1400);
  setTimeout(function () {
    put(rev[2], end[2]);
    put(rev[3], end[3]);
  }, 2600);
})();
