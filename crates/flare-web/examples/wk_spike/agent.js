// The agent's helper in every frame, in a content world of its own ("flare-agent"): the page cannot see
// or change it, and it cannot see the page's own JavaScript. It stands in for what Chromium's
// DevTools Protocol gives on Windows: a node number per element (backendDOMNodeId), an accessibility
// tree in the shape of Accessibility.getFullAXTree, the boxes of DOM.getContentQuads, focus and
// scrolling. Installed at document start, for all frames, by a WKUserScript.
(function () {
  'use strict';
  if (window.__flareAgent) return;
  var next = 1;
  var ids = new WeakMap();
  var nodes = new Map();
  function idOf(n) {
    var i = ids.get(n);
    if (!i) { i = next++; ids.set(n, i); nodes.set(i, new WeakRef(n)); }
    return i;
  }
  function nodeOf(i) { var r = nodes.get(i); return r ? r.deref() : undefined; }

  function clean(s) { return (s || '').replace(/\s+/g, ' ').trim(); }
  function text(el) { return clean(el.innerText !== undefined ? el.innerText : el.textContent); }

  function hidden(el) {
    if (el.hidden) return true;
    if (el.getAttribute('aria-hidden') === 'true') return true;
    if (el.checkVisibility) return !el.checkVisibility({visibilityProperty: true, contentVisibilityAuto: false});
    var cs = getComputedStyle(el);
    return cs.display === 'none' || cs.visibility === 'hidden';
  }

  var FROM_CONTENT = {button: 1, link: 1, heading: 1, option: 1, menuitem: 1, menuitemcheckbox: 1,
    menuitemradio: 1, tab: 1, treeitem: 1, checkbox: 1, radio: 1, switch: 1, DisclosureTriangle: 1, cell: 1};

  function roleOf(el) {
    var r = (el.getAttribute('role') || '').trim().split(/\s+/)[0];
    if (r) return r;
    var t = el.localName;
    switch (t) {
      case 'a': case 'area': return el.hasAttribute('href') ? 'link' : 'generic';
      case 'button': return 'button';
      case 'select': return (el.multiple || el.size > 1) ? 'listbox' : 'combobox';
      case 'option': return 'option';
      case 'textarea': return 'textbox';
      case 'img': return el.getAttribute('alt') === '' ? 'none' : 'image';
      case 'h1': case 'h2': case 'h3': case 'h4': case 'h5': case 'h6': return 'heading';
      case 'summary': return 'DisclosureTriangle';
      case 'dialog': return 'dialog';
      case 'iframe': case 'frame': return 'Iframe';
      case 'input':
        switch ((el.type || 'text').toLowerCase()) {
          case 'checkbox': return 'checkbox';
          case 'radio': return 'radio';
          case 'range': return 'slider';
          case 'number': return 'spinbutton';
          case 'search': return 'searchbox';
          case 'color': return 'ColorWell';
          case 'hidden': return 'none';
          case 'submit': case 'reset': case 'button': case 'image': case 'file': return 'button';
          default: return 'textbox';
        }
    }
    if (el.isContentEditable && (!el.parentElement || !el.parentElement.isContentEditable)) return 'textbox';
    return 'generic';
  }

  function byIdIn(el, i) {
    var root = el.getRootNode();
    return (root.getElementById ? root.getElementById(i) : null) || document.getElementById(i);
  }

  function nameOf(el, role) {
    var lb = el.getAttribute('aria-labelledby');
    if (lb) {
      var t = lb.split(/\s+/).map(function (i) { var r = byIdIn(el, i); return r ? text(r) : ''; }).join(' ').trim();
      if (t) return t;
    }
    var al = el.getAttribute('aria-label');
    if (al && al.trim()) return clean(al);
    if (el.labels && el.labels.length) {
      var l = Array.prototype.map.call(el.labels, text).join(' ').trim();
      if (l) return l;
    }
    var tag = el.localName;
    if ((tag === 'img' || tag === 'area' || (tag === 'input' && el.type === 'image')) && el.getAttribute('alt')) return clean(el.getAttribute('alt'));
    if (tag === 'input' && (el.type === 'submit' || el.type === 'reset' || el.type === 'button')) {
      return el.value || (el.type === 'submit' ? 'Submit' : el.type === 'reset' ? 'Reset' : '');
    }
    if (tag === 'input' && el.type === 'file') return 'Choose File';
    if (tag === 'iframe' && el.title) return clean(el.title);
    if (FROM_CONTENT[role]) { var c = text(el); if (c) return c; }
    if (el.title) return clean(el.title);
    var ph = el.getAttribute('placeholder');
    if (ph) return clean(ph);
    return '';
  }

  function deepActive() {
    var a = document.activeElement;
    while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement;
    return a;
  }

  function prop(list, name, value) { list.push({name: name, value: {type: typeof value, value: value}}); }

  function snapshot() {
    var out = [];
    var frames = [];
    var active = deepActive();
    var root = {nodeId: String(idOf(document)), backendDOMNodeId: idOf(document), ignored: false,
      role: {value: 'RootWebArea'}, name: {value: document.title || ''}, childIds: []};
    out.push(root);
    var budget = 20000;

    function emit(parent, n) {
      if (budget-- <= 0) return;
      if (n.nodeType === 3) {
        var t = clean(n.data);
        if (!t) return;
        var tn = {nodeId: String(idOf(n)), backendDOMNodeId: idOf(n), ignored: false,
          role: {value: 'StaticText'}, name: {value: t}, childIds: []};
        out.push(tn);
        parent.childIds.push(tn.nodeId);
        return;
      }
      if (n.nodeType !== 1) return;
      var el = n;
      var tag = el.localName;
      if (tag === 'script' || tag === 'style' || tag === 'template' || tag === 'noscript' || tag === 'head') return;
      if (hidden(el)) return;
      var role = roleOf(el);
      var me = parent;
      if (role !== 'generic' && role !== 'none' && role !== 'presentation') {
        var props = [];
        var node = {nodeId: String(idOf(el)), backendDOMNodeId: idOf(el), ignored: false,
          role: {value: role}, name: {value: nameOf(el, role)}, properties: props, childIds: []};
        if (el.disabled || el.getAttribute('aria-disabled') === 'true') prop(props, 'disabled', true);
        if (el.required || el.getAttribute('aria-required') === 'true') prop(props, 'required', true);
        if (el.readOnly || el.getAttribute('aria-readonly') === 'true') prop(props, 'readonly', true);
        if (el.getAttribute('aria-invalid') === 'true') prop(props, 'invalid', true);
        if (el === active) prop(props, 'focused', true);
        if (tag === 'input' && (el.type === 'checkbox' || el.type === 'radio')) prop(props, 'checked', el.indeterminate ? 'mixed' : el.checked);
        else if (el.hasAttribute('aria-checked')) prop(props, 'checked', el.getAttribute('aria-checked'));
        if (el.hasAttribute('aria-pressed')) prop(props, 'pressed', el.getAttribute('aria-pressed'));
        if (el.hasAttribute('aria-expanded')) prop(props, 'expanded', el.getAttribute('aria-expanded') === 'true');
        else if (tag === 'summary' && el.parentElement && el.parentElement.localName === 'details') prop(props, 'expanded', el.parentElement.open);
        if ((tag === 'option' && el.selected) || el.getAttribute('aria-selected') === 'true') prop(props, 'selected', true);
        if (role === 'heading') prop(props, 'level', tag.length === 2 && tag[0] === 'h' ? +tag[1] : (+el.getAttribute('aria-level') || 2));
        if (role === 'link' && el.href) prop(props, 'url', String(el.href));
        var v;
        if (tag === 'input' || tag === 'textarea') v = (el.type === 'password') ? '•'.repeat(el.value.length) : el.value;
        else if (tag === 'select') v = el.selectedOptions && el.selectedOptions[0] ? text(el.selectedOptions[0]) : '';
        else if (role === 'textbox' && el.isContentEditable) v = text(el);
        if (v !== undefined && v !== '') node.value = {value: v};
        if (role === 'Iframe') frames.push({id: idOf(el)});
        out.push(node);
        parent.childIds.push(node.nodeId);
        me = node;
        if (role === 'textbox' || role === 'combobox' || role === 'listbox' && tag === 'select') return;
      }
      var kids;
      if (el.shadowRoot) kids = el.shadowRoot.childNodes;
      else if (tag === 'slot') kids = el.assignedNodes({flatten: true});
      else kids = el.childNodes;
      if (!kids || kids.length === 0) {
        if (tag === 'slot') kids = el.childNodes; else return;
      }
      for (var i = 0; i < kids.length; i++) emit(me, kids[i]);
    }
    if (document.body) emit(root, document.body);
    return {nodes: out, frames: frames, truncated: budget <= 0};
  }

  function boxesOf(n) {
    if (!n) return null;
    var rects;
    if (n.nodeType === 3) { var r = document.createRange(); r.selectNodeContents(n); rects = r.getClientRects(); }
    else if (n.nodeType === 9) rects = [document.documentElement.getBoundingClientRect()];
    else rects = n.getClientRects();
    var q = [];
    for (var i = 0; i < rects.length; i++) {
      var b = rects[i];
      if (b.width <= 0 || b.height <= 0) continue;
      q.push([b.left, b.top, b.right, b.top, b.right, b.bottom, b.left, b.bottom]);
    }
    return q;
  }

  // The content box of an iframe element: where its document's (0, 0) sits in this frame.
  function frameOrigin(i) {
    var el = nodeOf(i);
    if (!el) return null;
    var b = el.getBoundingClientRect();
    return [b.left + el.clientLeft, b.top + el.clientTop];
  }

  function scrollInto(i) {
    var n = nodeOf(i);
    if (!n) return 'gone';
    var el = n.nodeType === 1 ? n : n.parentElement;
    if (!el) return 'no element';
    var b = el.getBoundingClientRect();
    var inView = b.top >= 0 && b.left >= 0 && b.bottom <= innerHeight && b.right <= innerWidth;
    if (!inView) el.scrollIntoView({block: 'center', inline: 'center', behavior: 'instant'});
    return 'ok';
  }

  function focusNode(i) {
    var n = nodeOf(i);
    if (!n || n.nodeType !== 1) return 'Element is not focusable';
    n.focus({preventScroll: false});
    var a = deepActive();
    return a === n ? 'ok' : 'Element is not focusable';
  }

  // Trusted input events this frame has seen: the shim waits for the count to move after it sends an
  // event, so an Input.* call is answered once the page has handled it (as CDP's are).
  var seen = 0;
  ['mousedown', 'mouseup', 'mousemove', 'click', 'auxclick', 'keydown', 'keyup', 'wheel', 'beforeinput', 'input'].forEach(function (t) {
    window.addEventListener(t, function (e) { if (e.isTrusted) seen++; }, true);
  });

  // The last keydown and whether the page cancelled it (read after the page's own listeners ran): a
  // Command chord the page did not take becomes its editing action (select all, copy, paste...).
  var lastKey = null;
  window.addEventListener('keydown', function (e) {
    if (e.isTrusted) lastKey = {key: e.key, meta: e.metaKey, ctrl: e.ctrlKey};
  }, true);
  window.addEventListener('keydown', function (e) {
    if (e.isTrusted && lastKey) lastKey.prevented = e.defaultPrevented;
  }, false);

  // A parent frame's agent asks "which frame are you?": the answer goes to Rust with this frame's
  // WKFrameInfo attached. Rust checks the nonce it made for this look.
  var probed = null;
  window.addEventListener('message', function (e) {
    var d = e.data;
    if (d && typeof d === 'object' && typeof d.__flareProbe === 'string') {
      probed = d.__flareProbe;
      window.webkit.messageHandlers.flareAgent.postMessage(JSON.stringify({kind: 'probe', nonce: d.__flareProbe, key: d.key, href: location.href}));
    }
  }, true);

  window.__flareAgent = {
    seen: function () { return seen; },
    probed: function () { return probed; },
    lastKey: function () { var k = lastKey; lastKey = null; return JSON.stringify(k); },
    snapshot: function () { return JSON.stringify(snapshot()); },
    quads: function (i) { return JSON.stringify(boxesOf(nodeOf(i))); },
    frameOrigin: function (i) { return JSON.stringify(frameOrigin(i)); },
    // The size of an iframe's viewport, to pair it with a frame that cannot answer the probe.
    frameSize: function (i) { var el = nodeOf(i); return JSON.stringify(el ? [el.clientWidth, el.clientHeight] : null); },
    scrollInto: function (i) { return scrollInto(i); },
    focus: function (i) { return focusNode(i); },
    // Does `src` parse as one expression? Compiled here, never called, so nothing of it runs. The page's
    // CSP does not govern this world (null means the check itself was refused).
    parsesAsExpression: function (src) {
      var AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
      try { new AsyncFunction('return await (\n' + src + '\n);'); return true; } catch (e) {
        return (e instanceof SyntaxError) ? false : null;
      }
    },
    // A statement script split into its statements and its last expression statement, so the host can
    // run the statements as a script (globals as CDP makes them) and then await the expression: CDP's
    // awaitPromise on a completion value. The longest expression that ends the script wins; a split
    // inside a string or comment leaves a prefix that does not parse. Null when there is none.
    splitLast: function (src) {
      if (src.length > 20000) return null;
      var AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
      function parses(make) { try { make(); return true; } catch (e) { return false; } }
      for (var i = 1; i < src.length; i++) {
        var c = src[i - 1];
        if (c !== ';' && c !== '}' && c !== '\n') continue;
        var head = src.slice(0, i), tail = src.slice(i);
        if (!tail.trim()) continue;
        if (!parses(function () { new AsyncFunction('return await (\n' + tail + '\n);'); })) continue;
        if (!parses(function () { new Function(head); })) continue;
        return JSON.stringify({head: head, tail: tail});
      }
      return null;
    },
    probe: function (i, nonce, key) {
      var el = nodeOf(i);
      if (!el || !el.contentWindow) return 'no frame';
      el.contentWindow.postMessage({__flareProbe: nonce, key: key}, '*');
      return 'sent';
    },
    metrics: function () {
      var v = window.visualViewport;
      return JSON.stringify({
        cssVisualViewport: {clientWidth: v ? v.width : innerWidth, clientHeight: v ? v.height : innerHeight,
          pageX: v ? v.pageLeft : scrollX, pageY: v ? v.pageTop : scrollY, scale: v ? v.scale : 1},
        cssLayoutViewport: {clientWidth: document.documentElement.clientWidth, clientHeight: document.documentElement.clientHeight,
          pageX: scrollX, pageY: scrollY},
        cssContentSize: {width: document.documentElement.scrollWidth, height: document.documentElement.scrollHeight}
      });
    }
  };
})();
