// ForkTTY browser-pane scripting driver (SP2). Injected as a persistent
// WebKit user script at document-start, so window.__forktty is present on
// every page and after every navigation. Idempotent: re-running keeps state.
(function () {
  if (window.__forktty) return;

  var refMap = new Map();
  var counter = 0;

  function isHidden(el) {
    if (el.hidden) return true;
    if (el.getAttribute && el.getAttribute("aria-hidden") === "true") return true;
    var s = window.getComputedStyle(el);
    return s.display === "none" || s.visibility === "hidden";
  }

  function roleOf(el) {
    var r = el.getAttribute && el.getAttribute("role");
    if (r) return r;
    var tag = el.tagName.toLowerCase();
    if (tag === "input") {
      var t = (el.getAttribute("type") || "text").toLowerCase();
      if (t === "checkbox") return "checkbox";
      if (t === "radio") return "radio";
      if (t === "button" || t === "submit" || t === "reset") return "button";
      if (t === "hidden") return "";
      return "textbox";
    }
    var implicit = {
      a: "link", button: "button", textarea: "textbox",
      select: "combobox", h1: "heading", h2: "heading", h3: "heading",
      h4: "heading", h5: "heading", h6: "heading", nav: "navigation",
      main: "main", img: "img", form: "form"
    };
    return implicit[tag] || "";
  }

  function nameOf(el) {
    var label = el.getAttribute && el.getAttribute("aria-label");
    if (label) return label.trim();
    var ph = el.getAttribute && el.getAttribute("placeholder");
    if (ph) return ph.trim();
    if (el.tagName === "INPUT" || el.tagName === "TEXTAREA") {
      return (el.value || "").trim();
    }
    var text = (el.textContent || "").trim();
    return text.length > 120 ? text.slice(0, 120) : text;
  }

  function isInteresting(el) {
    return roleOf(el) !== "";
  }

  function isTextInput(el) {
    if (!(el instanceof HTMLInputElement)) return false;
    var t = (el.getAttribute("type") || "text").toLowerCase();
    return ![
      "button", "checkbox", "file", "hidden", "image", "radio", "reset", "submit"
    ].includes(t);
  }

  function isReadOnly(el) {
    return !!(
      el.readOnly ||
      (el.getAttribute && el.getAttribute("aria-readonly") === "true")
    );
  }

  function isLiveElement(el) {
    return !!(el && el.isConnected && document.contains(el));
  }

  function isClickable(el) {
    if (!isLiveElement(el)) return false;
    if (el.disabled || isHidden(el)) return false;
    return typeof el.click === "function";
  }

  function isFillable(el) {
    if (!isLiveElement(el)) return false;
    if (el.disabled || isReadOnly(el) || isHidden(el)) return false;
    return isTextInput(el) ||
      el instanceof HTMLTextAreaElement ||
      el instanceof HTMLSelectElement ||
      el.isContentEditable;
  }

  function nodeFor(el) {
    var ref = "e" + (++counter);
    refMap.set(ref, el);
    return {
      ref: ref,
      role: roleOf(el),
      name: nameOf(el),
      value: el.value !== undefined ? String(el.value) : "",
      children: walkChildren(el)
    };
  }

  function walkChildren(el) {
    var out = [];
    var kids = el.children || [];
    for (var i = 0; i < kids.length; i++) {
      var child = kids[i];
      if (isHidden(child)) continue;
      if (isInteresting(child)) {
        out.push(nodeFor(child));
      } else {
        var sub = walkChildren(child);
        for (var j = 0; j < sub.length; j++) out.push(sub[j]);
      }
    }
    return out;
  }

  window.__forktty = {
    snapshot: function () {
      refMap = new Map();
      counter = 0;
      var children = document.body ? walkChildren(document.body) : [];
      var root = { role: "document", name: "", value: "", children: children };
      return root;
    },
    click: function (ref) {
      var el = refMap.get(ref);
      if (!el) throw new Error("ref-not-found");
      if (!isLiveElement(el)) throw new Error("ref-not-found");
      if (!isClickable(el)) throw new Error("ref-not-interactable");
      el.scrollIntoView({ block: "center" });
      el.click();
      return true;
    },
    fill: function (ref, value) {
      var el = refMap.get(ref);
      if (!el) throw new Error("ref-not-found");
      if (!isLiveElement(el)) throw new Error("ref-not-found");
      if (!isFillable(el)) throw new Error("ref-not-interactable");
      el.focus();
      if (el.isContentEditable) {
        el.textContent = value;
      } else {
        el.value = value;
      }
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    }
  };
})();
