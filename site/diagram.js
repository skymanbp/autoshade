(function () {
  "use strict";

  // 1x is the diagram's own viewBox and is also the floor: line art has nothing
  // around it, so zooming out past the frame would only add empty margin. 8x is
  // the ceiling, at which the smallest 11px label reads like a headline.
  var MAX_SCALE = 8;
  // The + and - buttons and the +/- keys step by this factor, a double-click by
  // DOUBLE_STEP. Both are geometric, so N steps in followed by N steps out land
  // back on exactly the zoom the reader started from.
  var STEP = 1.4;
  var DOUBLE_STEP = 2;
  // Arrow keys move the window by this fraction of itself: small enough to read
  // by, large enough to cross the whole diagram in about a dozen presses.
  var PAN_FRACTION = 0.08;
  // A mouse notch reports deltaY = 100, and 100 x 0.0015 in the exponent below
  // is an 11% zoom -- fine enough that a trackpad feels continuous, coarse
  // enough that one notch of a wheel visibly moves.
  var WHEEL_GAIN = 0.0015;
  // Some systems report wheel deltas in lines or pages instead of pixels; these
  // are the conventional pixel equivalents, so the gain above means the same
  // thing on every machine.
  var LINE_PIXELS = 16;
  var PAGE_PIXELS = 400;

  // The viewBox as four numbers, or null when the attribute is missing or
  // malformed. Everything downstream treats null as "this figure is not a
  // diagram", which is how a figure whose SVG has not been spliced in yet stays
  // inert instead of throwing on every wheel event.
  function parseViewBox(svg) {
    var raw = svg.getAttribute("viewBox");
    if (!raw) {
      return null;
    }
    var parts = raw.trim().split(/[\s,]+/);
    if (parts.length !== 4) {
      return null;
    }
    var box = {
      x: parseFloat(parts[0]),
      y: parseFloat(parts[1]),
      w: parseFloat(parts[2]),
      h: parseFloat(parts[3])
    };
    if (!isFinite(box.x) || !isFinite(box.y) || !(box.w > 0) || !(box.h > 0)) {
      return null;
    }
    return box;
  }

  function setUp(figure) {
    var stage = figure.querySelector(".diagram-stage");
    var svg = stage ? stage.querySelector("svg") : null;
    var base = svg ? parseViewBox(svg) : null;
    if (!base) {
      return;
    }

    var controls = figure.querySelector(".diagram-controls");
    var readout = figure.querySelector('[data-dg="readout"]');
    var fullscreenButton = figure.querySelector('[data-dg="fullscreen"]');
    // The window currently on screen, in the diagram's own units. Its aspect
    // ratio is held equal to the diagram's for the life of the page, so the SVG
    // never letterboxes itself as a side effect of zooming.
    var view = { x: base.x, y: base.y, w: base.w, h: base.h };
    var aspect = base.h / base.w;
    // Whether the reader has taken hold of this diagram; see the wheel handler.
    var engaged = false;
    // Live pointers by pointerId, and the finger spread at the last pinch
    // sample. Two entries mean a pinch, one means a drag.
    var pointers = {};
    var spread = 0;

    function ids() {
      return Object.keys(pointers);
    }

    // Keep the window inside the drawing -- never wider than the whole of it,
    // never narrower than 1/MAX_SCALE of it, and never panned so far that an
    // edge of the drawing crosses an edge of the window. Clamping the origin
    // against (base + size - window) is what makes it impossible to drag the
    // content out of sight.
    function clamp() {
      view.w = Math.min(base.w, Math.max(base.w / MAX_SCALE, view.w));
      view.h = view.w * aspect;
      view.x = Math.min(Math.max(view.x, base.x), base.x + base.w - view.w);
      view.y = Math.min(Math.max(view.y, base.y), base.y + base.h - view.h);
    }

    function round(value) {
      return Math.round(value * 1000) / 1000;
    }

    function render() {
      clamp();
      svg.setAttribute("viewBox", round(view.x) + " " + round(view.y) + " " +
        round(view.w) + " " + round(view.h));
      if (readout) {
        // Only write when the whole percent actually changed: the readout is a
        // live region, and a drag would otherwise announce the same number to a
        // screen reader on every pointer sample.
        var percent = Math.round((base.w / view.w) * 100) + "%";
        if (readout.textContent !== percent) {
          readout.textContent = percent;
        }
      }
    }

    // Client pixels to diagram units. The screen matrix already carries both
    // the viewBox scale and the centring that preserveAspectRatio adds when the
    // stage is taller than the drawing, so this is the one conversion that is
    // right on the index page (where the SVG fills its stage) and on the
    // architecture page (where it is fitted inside a taller one).
    function toUser(clientX, clientY) {
      var m = svg.getScreenCTM();
      if (!m || !m.a || !m.d) {
        return null;
      }
      return { x: (clientX - m.e) / m.a, y: (clientY - m.f) / m.d };
    }

    // Diagram units per client pixel, for turning a drag into a pan. This is
    // view.w / (rendered width) -- the same ratio as viewBox width over stage
    // width whenever the SVG fills the stage, and the correct one when it does
    // not.
    function unitsPerPixel() {
      var m = svg.getScreenCTM();
      if (!m || !m.a || !m.d) {
        return null;
      }
      return { x: 1 / m.a, y: 1 / m.d };
    }

    // Scale by `factor` about the diagram point (ax, ay): that point keeps its
    // fractional position inside the window, which is what makes the pixel
    // under the cursor stay under the cursor.
    function zoomAt(factor, ax, ay) {
      var width = Math.min(base.w, Math.max(base.w / MAX_SCALE, view.w / factor));
      if (width === view.w) {
        return;
      }
      var fx = (ax - view.x) / view.w;
      var fy = (ay - view.y) / view.h;
      view.x = ax - fx * width;
      view.y = ay - fy * width * aspect;
      view.w = width;
      view.h = width * aspect;
      render();
    }

    function zoomAtCentre(factor) {
      zoomAt(factor, view.x + view.w / 2, view.y + view.h / 2);
    }

    function reset() {
      view.x = base.x;
      view.y = base.y;
      view.w = base.w;
      view.h = base.h;
      render();
    }

    // Engagement. Zooming takes the wheel away from the page, so the diagram
    // only claims it once the reader has taken hold of this figure -- pressed a
    // pointer inside it or moved the keyboard focus into it. A reader scrolling
    // past keeps scrolling past. Ctrl/Cmd + wheel is the exception because that
    // gesture already means "zoom" everywhere else.
    function engage() {
      engaged = true;
    }

    function releaseIfOutside(event) {
      if (!figure.contains(event.target)) {
        engaged = false;
      }
    }

    figure.addEventListener("pointerdown", engage);
    figure.addEventListener("focusin", engage);
    // Capture phase, so this runs before the figure's own handler and a press
    // inside the figure re-engages it rather than being disengaged by it.
    document.addEventListener("pointerdown", releaseIfOutside, true);
    document.addEventListener("focusin", releaseIfOutside, true);

    stage.addEventListener("wheel", function (event) {
      if (!engaged && !event.ctrlKey && !event.metaKey) {
        return;
      }
      var anchor = toUser(event.clientX, event.clientY);
      if (!anchor) {
        return;
      }
      event.preventDefault();
      var delta = event.deltaY;
      if (event.deltaMode === 1) {
        delta *= LINE_PIXELS;
      } else if (event.deltaMode === 2) {
        delta *= PAGE_PIXELS;
      }
      zoomAt(Math.pow(2, -delta * WHEEL_GAIN), anchor.x, anchor.y);
    }, { passive: false });

    stage.addEventListener("pointerdown", function (event) {
      if (event.pointerType === "mouse" && event.button !== 0) {
        return;
      }
      pointers[event.pointerId] = { x: event.clientX, y: event.clientY };
      if (ids().length === 2) {
        spread = fingerSpread();
      }
      try {
        stage.setPointerCapture(event.pointerId);
      } catch (error) {
        // A pointer that ended between the event and this call cannot be
        // captured; the pointerup handler below cleans up either way.
      }
      stage.classList.add("is-dragging");
      // A drag across the diagram is a pan, not a text selection or an image
      // drag -- but preventDefault also cancels the click's focus, so the stage
      // asks for focus itself and keeps the keyboard shortcuts reachable.
      event.preventDefault();
      if (document.activeElement !== stage) {
        stage.focus({ preventScroll: true });
      }
    });

    function fingerSpread() {
      var live = ids();
      if (live.length < 2) {
        return 0;
      }
      var a = pointers[live[0]];
      var b = pointers[live[1]];
      return Math.sqrt((b.x - a.x) * (b.x - a.x) + (b.y - a.y) * (b.y - a.y));
    }

    function fingerMidpoint() {
      var live = ids();
      var a = pointers[live[0]];
      var b = pointers[live[1]];
      return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
    }

    stage.addEventListener("pointermove", function (event) {
      var previous = pointers[event.pointerId];
      if (!previous) {
        return;
      }
      var current = { x: event.clientX, y: event.clientY };
      pointers[event.pointerId] = current;

      if (ids().length >= 2) {
        // Pinch: the ratio of the finger spread is the zoom, and the point
        // between the fingers is what stays put.
        var now = fingerSpread();
        var mid = fingerMidpoint();
        var anchor = toUser(mid.x, mid.y);
        if (spread > 0 && now > 0 && anchor) {
          zoomAt(now / spread, anchor.x, anchor.y);
        }
        spread = now;
        return;
      }

      var unit = unitsPerPixel();
      if (!unit) {
        return;
      }
      view.x -= (current.x - previous.x) * unit.x;
      view.y -= (current.y - previous.y) * unit.y;
      render();
    });

    function endPointer(event) {
      if (!pointers[event.pointerId]) {
        return;
      }
      delete pointers[event.pointerId];
      if (ids().length < 2) {
        spread = 0;
      }
      if (ids().length === 0) {
        stage.classList.remove("is-dragging");
      }
      if (stage.hasPointerCapture && stage.hasPointerCapture(event.pointerId)) {
        stage.releasePointerCapture(event.pointerId);
      }
    }

    stage.addEventListener("pointerup", endPointer);
    stage.addEventListener("pointercancel", endPointer);

    stage.addEventListener("dblclick", function (event) {
      var anchor = toUser(event.clientX, event.clientY);
      if (!anchor) {
        return;
      }
      event.preventDefault();
      var out = event.shiftKey || event.altKey;
      zoomAt(out ? 1 / DOUBLE_STEP : DOUBLE_STEP, anchor.x, anchor.y);
    });

    stage.addEventListener("keydown", function (event) {
      if (event.ctrlKey || event.metaKey || event.altKey) {
        return;
      }
      switch (event.key) {
      case "+":
      case "=":
        zoomAtCentre(STEP);
        break;
      case "-":
      case "_":
        zoomAtCentre(1 / STEP);
        break;
      case "0":
        reset();
        break;
      case "ArrowLeft":
        view.x -= view.w * PAN_FRACTION;
        render();
        break;
      case "ArrowRight":
        view.x += view.w * PAN_FRACTION;
        render();
        break;
      case "ArrowUp":
        view.y -= view.h * PAN_FRACTION;
        render();
        break;
      case "ArrowDown":
        view.y += view.h * PAN_FRACTION;
        render();
        break;
      default:
        // Every other key keeps its ordinary meaning, Tab and Escape included,
        // so returning here rather than falling through to preventDefault is
        // what keeps the focused stage from swallowing the page's keyboard.
        return;
      }
      event.preventDefault();
    });

    function toggleFullscreen() {
      if (document.fullscreenElement === figure) {
        if (document.exitFullscreen) {
          document.exitFullscreen();
        }
        return;
      }
      if (!figure.requestFullscreen) {
        return;
      }
      var request = figure.requestFullscreen();
      if (request && request.catch) {
        // A browser that refuses the request leaves the page exactly as it was,
        // which is a usable outcome and not worth an error dialog.
        request.catch(function () { return undefined; });
      }
    }

    // The three pillar figures ask for full screen through the Fullscreen API;
    // the architecture figure on the home page uses a plain link to
    // architecture.html instead and has no button to hide. A browser without
    // the unprefixed API -- which is also a browser without the unprefixed
    // :fullscreen selector the stylesheet uses -- gets no button rather than
    // one that does nothing.
    if (fullscreenButton &&
        !(document.fullscreenEnabled && figure.requestFullscreen)) {
      fullscreenButton.hidden = true;
    }

    figure.addEventListener("click", function (event) {
      var target = event.target.closest ? event.target.closest("[data-dg]") : null;
      if (!target || !figure.contains(target)) {
        return;
      }
      switch (target.getAttribute("data-dg")) {
      case "in":
        zoomAtCentre(STEP);
        break;
      case "out":
        zoomAtCentre(1 / STEP);
        break;
      case "reset":
        reset();
        break;
      case "fullscreen":
        toggleFullscreen();
        break;
      default:
        break;
      }
    });

    // The controls carry the `hidden` attribute in the markup and lose it here,
    // so a reader with JavaScript switched off is never shown four buttons and
    // a zoom readout that cannot do anything.
    if (controls) {
      controls.hidden = false;
    }
    render();
  }

  var figures = document.querySelectorAll(".diagram-figure");
  for (var i = 0; i < figures.length; i += 1) {
    setUp(figures[i]);
  }
})();
