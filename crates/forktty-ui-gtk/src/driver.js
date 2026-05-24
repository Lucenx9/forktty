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
    var implicit = {
      a: "link", button: "button", input: "textbox", textarea: "textbox",
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

  function walk(el) {
    var node = null;
    if (isInteresting(el)) {
      var ref = "e" + (++counter);
      refMap.set(ref, el);
      node = {
        ref: ref,
        role: roleOf(el),
        name: nameOf(el),
        value: el.value !== undefined ? String(el.value) : "",
        children: []
      };
    }
    var kids = el.children || [];
    for (var i = 0; i < kids.length; i++) {
      var child = kids[i];
      if (isHidden(child)) continue;
      var childNode = walk(child);
      if (childNode) {
        if (node) node.children.push(childNode);
        else return childNode; // collapse: surface descendant when parent uninteresting
      }
    }
    return node;
  }

  window.__forktty = {
    snapshot: function () {
      refMap = new Map();
      counter = 0;
      var root = walk(document.body) || { role: "document", name: "", value: "", children: [] };
      return JSON.stringify(root);
    },
    click: function (ref) {
      var el = refMap.get(ref);
      if (!el) throw "ref-not-found";
      el.scrollIntoView({ block: "center" });
      el.click();
      return true;
    },
    fill: function (ref, value) {
      var el = refMap.get(ref);
      if (!el) throw "ref-not-found";
      el.focus();
      el.value = value;
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    }
  };
})();
