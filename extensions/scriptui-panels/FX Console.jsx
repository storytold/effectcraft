/**
 * FX Console for EffectCraft
 *
 * Standalone ScriptUI floating effect launcher recreating the Video Copilot FX Console design.
 *
 * Target Dimensions: ~396px wide, ~40px tall (State A), ~125px tall (State B, 2 results).
 *
 * STATE A (Initial / Idle State):
 *   Compact floating command bar showing only the search icon, search input,
 *   and five toolbar icons (Camera, Gallery, Copy, Settings, Action Logo).
 *   Search field receives focus immediately upon opening.
 *
 * STATE B (Active Search State):
 *   When typing, dynamically expands downward to display the category header
 *   ("● Effects") and matching effect rows. The active result is highlighted
 *   with a full-width vibrant blue background (#1372b6).
 *
 * (c) EffectCraft ScriptUI Extension
 */

(function fxConsole(thisObj) {
  // ------------------------------------------------------------------
  // 1. Instance Management (Prevent duplicate windows & clean reopen)
  // ------------------------------------------------------------------
  var globalScope = (function () {
    return this;
  })();

  // Reuse existing open instance if available
  if (globalScope.__fxConsoleInstance && globalScope.__fxConsoleInstance.win) {
    var existing = globalScope.__fxConsoleInstance;
    try {
      if (existing.win && !existing.win.__closed && existing.win.visible) {
        if (existing.searchField) {
          existing.searchField.text = "";
        }
        if (typeof existing.update === "function") {
          existing.update("");
        }
        if (typeof existing.activate === "function") {
          existing.activate();
        }
        return;
      }
    } catch (e) {
      // If check fails, clean up old instance
    }
    try {
      existing.win.close();
    } catch (e) { }
    globalScope.__fxConsoleInstance = null;
  }


  // ------------------------------------------------------------------
  // 2. Effect Catalogue Cache & Retrieval
  // ------------------------------------------------------------------
  var cachedCatalogue = globalScope.__fxConsoleCatalogueCache || null;

  function loadEffectCatalogue() {
    if (cachedCatalogue && cachedCatalogue.length > 0) {
      return cachedCatalogue;
    }

    try {
      var rawList = app.run("effect.list", {});
      if (rawList && rawList instanceof Array && rawList.length > 0) {
        cachedCatalogue = [];
        for (var i = 0; i < rawList.length; i++) {
          var item = rawList[i];
          if (item && item.id && item.name) {
            cachedCatalogue.push({
              id: String(item.id),
              name: String(item.name),
              category: String(item.category || "Other")
            });
          }
        }
        globalScope.__fxConsoleCatalogueCache = cachedCatalogue;
        return cachedCatalogue;
      }
    } catch (err) {
      writeLn("FX Console: Notice - effect.list command failed: " + err.toString());
    }

    return [];
  }

  // ------------------------------------------------------------------
  // 3. Search Matching & Ranking Algorithm
  // ------------------------------------------------------------------
  function scoreMatch(effect, query) {
    var nameLower = effect.name.toLowerCase();
    var catLower = (effect.category || "").toLowerCase();
    var idLower = effect.id.toLowerCase();
    var q = query.toLowerCase();
    var score = 0;

    // 1. Exact or prefix name matches
    if (nameLower === q) {
      score += 1000;
    } else if (nameLower.indexOf(q) === 0) {
      score += 800;
    } else if (nameLower.indexOf(" " + q) >= 0) {
      score += 600;
    } else {
      var nameIdx = nameLower.indexOf(q);
      if (nameIdx > 0) {
        score += 400 - Math.min(nameIdx, 50);
      }
    }

    // 2. Category matches
    if (catLower === q) {
      score += 300;
    } else if (catLower.indexOf(q) === 0) {
      score += 200;
    } else if (catLower.indexOf(q) > 0) {
      score += 100;
    }

    // 3. Effect ID matches
    if (idLower.indexOf(q) >= 0) {
      score += 50;
    }

    return score;
  }

  function filterEffects(query) {
    var catalogue = loadEffectCatalogue();
    if (!catalogue || catalogue.length === 0) {
      return [];
    }

    var trimmed = String(query === undefined || query === null ? "" : query).replace(/^\s+|\s+$/g, "");
    if (trimmed.length === 0) {
      return [];
    }

    var scored = [];
    for (var i = 0; i < catalogue.length; i++) {
      var s = scoreMatch(catalogue[i], trimmed);
      if (s > 0) {
        scored.push({ effect: catalogue[i], score: s });
      }
    }

    scored.sort(function (a, b) {
      if (b.score !== a.score) {
        return b.score - a.score;
      }
      if (a.effect.name.length !== b.effect.name.length) {
        return a.effect.name.length - b.effect.name.length;
      }
      var an = a.effect.name.toLowerCase();
      var bn = b.effect.name.toLowerCase();
      return an < bn ? -1 : (an > bn ? 1 : 0);
    });

    var results = [];
    for (var j = 0; j < scored.length; j++) {
      results.push(scored[j].effect);
    }
    return results;
  }

  // ------------------------------------------------------------------
  // 4. Composition & Layer Helpers
  // ------------------------------------------------------------------
  function getActiveComp() {
    var item = app.project ? app.project.activeItem : null;
    if (item && item instanceof CompItem) {
      return item;
    }
    return null;
  }

  function getSelectedLayers(comp) {
    if (!comp) return [];
    var layers = comp.selectedLayers;
    if (!layers || !(layers instanceof Array) || layers.length === 0) {
      return [];
    }
    return layers;
  }

  // ------------------------------------------------------------------
  // 5. Layout Constants (Matching Reference Dimensions & Proportions)
  // ------------------------------------------------------------------
  var TARGET_WIDTH = 396;       // Exact reference console width
  var INITIAL_HEIGHT = 42;      // State A compact height (exact reference ~42px)
  var ICON_SIZE = 22;           // Toolbar button square dimension
  var SEARCH_ICON_W = 20;       // Magnifying glass width
  var ROW_SPACING = 3;          // Clean compact spacing
  var WIN_MARGIN = 2;           // Tight outer border padding
  var TOOLBAR_COUNT = 5;
  var TOOLBAR_W = (TOOLBAR_COUNT * ICON_SIZE) + ((TOOLBAR_COUNT - 1) * ROW_SPACING); // 5*22 + 4*3 = 122

  // ------------------------------------------------------------------
  // 6. Build ScriptUI Window
  // ------------------------------------------------------------------
  var win = (typeof Panel !== "undefined" && thisObj instanceof Panel) ? thisObj : new Window("palette", "FX Console", undefined, { resizeable: true });
  win.orientation = "column";
  win.alignChildren = ["fill", "top"];
  win.spacing = 0;
  win.margins = [WIN_MARGIN, WIN_MARGIN, WIN_MARGIN, WIN_MARGIN];
  win.preferredSize = [TARGET_WIDTH, INITIAL_HEIGHT];

  // Set dark charcoal background matching reference (#181818) and outer border (#e4c4b5)
  try {
    var winG = win.graphics;
    var bgBrush = winG.newBrush(winG.BrushType.SOLID_COLOR, [0.094, 0.094, 0.094, 1.0]);
    winG.backgroundColor = bgBrush;
  } catch (e) { }

  win.onDraw = function () {
    var g = win.graphics;
    var w = (win.size && win.size[0] > 0) ? win.size[0] : (win.bounds && win.bounds.width > 0 ? win.bounds.width : TARGET_WIDTH);
    var h = (win.size && win.size[1] > 0) ? win.size[1] : (win.bounds && win.bounds.height > 0 ? win.bounds.height : INITIAL_HEIGHT);

    // Dark charcoal window background (#181818)
    var bg = g.newBrush(g.BrushType.SOLID_COLOR, [0.094, 0.094, 0.094, 1.0]);
    g.rectPath(0, 0, w, h);
    g.fillPath(bg);

    // Subtle warm light-beige outer border (#e4c4b5)
    var borderPen = g.newPen(g.PenType.SOLID_COLOR, [0.89, 0.77, 0.71, 1.0], 1.0);
    g.rectPath(0.5, 0.5, w - 1, h - 1);
    g.strokePath(borderPen);
  };

  // ==================================================================
  // TOP ROW: Toolbar (Search Icon, Search Input, 5 Toolbar Buttons)
  // ==================================================================
  var topRow = win.add("group");
  topRow.orientation = "row";
  topRow.alignChildren = ["fill", "center"];
  topRow.spacing = ROW_SPACING;
  topRow.margins = [2, 0, 2, 0];
  topRow.preferredSize = [TARGET_WIDTH - (WIN_MARGIN * 2), 26];

  // 1. Search Icon (White magnifying glass vector)
  var searchIcon = topRow.add("group");
  searchIcon.preferredSize = [SEARCH_ICON_W, ICON_SIZE];
  searchIcon.helpTip = "Search effects";
  searchIcon.onDraw = function () {
    var g = searchIcon.graphics;
    var pen = g.newPen(g.PenType.SOLID_COLOR, [0.96, 0.96, 0.96, 1.0], 1.8);
    // Magnifying glass circular rim
    g.ellipsePath(3, 4, 11, 11);
    g.strokePath(pen);
    // Handle pointing down-right
    g.moveTo(12.5, 13.5);
    g.lineTo(17.5, 18.5);
    g.strokePath(pen);
  };

  // 2. Search Input Field (Dynamically occupies remaining horizontal space)
  var searchField = topRow.add("edittext", undefined, "", { placeholder: "Fx" });
  searchField.characters = 0; // Avoid rigid character width constraints
  searchField.helpTip = "Type an effect name (Enter to apply, Esc to close)";
  searchField.name = "searchField";
  try {
    searchField.graphics.font = ScriptUI.newFont("Segoe UI", ScriptUI.FontStyle.REGULAR, 15);
  } catch (e) { }

  // 3. Toolbar Icons Group (Right Aligned, pinned to the right edge)
  var toolbar = topRow.add("group");
  toolbar.orientation = "row";
  toolbar.alignChildren = ["right", "center"];
  toolbar.spacing = ROW_SPACING;
  toolbar.margins = [0, 0, 0, 0];
  toolbar.preferredSize = [TOOLBAR_W, ICON_SIZE];

  // Helper to create outline icon buttons
  function createIconButton(tipText, drawFn) {
    var btn = toolbar.add("button", undefined, "");
    btn.preferredSize = [ICON_SIZE, ICON_SIZE];
    btn.helpTip = tipText;
    btn.onDraw = function () {
      drawFn(btn.graphics);
    };
    return btn;
  }

  // Icon 1: Camera (Take Snapshot)
  var cameraBtn = createIconButton("Take Snapshot (Shift+F5)", function (g) {
    var brush = g.newBrush(g.BrushType.SOLID_COLOR, [0.72, 0.72, 0.72, 1.0]);
    var bg = g.newBrush(g.BrushType.SOLID_COLOR, [0.094, 0.094, 0.094, 1.0]);
    // Top flash / prism bump
    g.rectPath(7, 3, 7, 2.5);
    g.fillPath(brush);
    // Camera body
    g.rectPath(2, 5.5, 18, 12.5);
    g.fillPath(brush);
    // Outer lens cutout
    g.ellipsePath(7, 7.5, 8, 8);
    g.fillPath(bg);
    // Center lens element
    g.ellipsePath(9, 9.5, 4, 4);
    g.fillPath(brush);
    // Viewfinder dot at top right
    g.ellipsePath(16.5, 7.5, 2, 2);
    g.fillPath(bg);
  });

  // Icon 2: Gallery/Filmstrip (Show Snapshot)
  var galleryBtn = createIconButton("Show Snapshot", function (g) {
    var brush = g.newBrush(g.BrushType.SOLID_COLOR, [0.72, 0.72, 0.72, 1.0]);
    var bg = g.newBrush(g.BrushType.SOLID_COLOR, [0.094, 0.094, 0.094, 1.0]);
    // Left film perforations (3 blocks)
    g.rectPath(2, 4, 4, 3);
    g.fillPath(brush);
    g.rectPath(2, 9, 4, 3);
    g.fillPath(brush);
    g.rectPath(2, 14, 4, 3);
    g.fillPath(brush);
    // Right picture frame
    g.rectPath(7.5, 4, 13, 13);
    g.fillPath(brush);
    // Mountain / landscape cutout
    g.moveTo(9, 14);
    g.lineTo(13, 9.5);
    g.lineTo(16, 12.5);
    g.lineTo(17.5, 11);
    g.lineTo(19.5, 14);
    g.lineTo(9, 14);
    g.fillPath(bg);
    // Sun dot cutout
    g.ellipsePath(10.5, 6, 2.5, 2.5);
    g.fillPath(bg);
  });

  // Icon 3: Download/Tray (Copy Frame to Clipboard)
  var copyBtn = createIconButton("Copy Frame to Clipboard", function (g) {
    var brush = g.newBrush(g.BrushType.SOLID_COLOR, [0.72, 0.72, 0.72, 1.0]);
    // Vertical stem
    g.rectPath(9.5, 3, 3, 8);
    g.fillPath(brush);
    // Downward arrowhead
    g.moveTo(6.5, 10);
    g.lineTo(15.5, 10);
    g.lineTo(11, 14.5);
    g.lineTo(6.5, 10);
    g.fillPath(brush);
    // Bottom tray
    g.rectPath(3, 15, 16, 2.5);
    g.fillPath(brush);
    g.rectPath(3, 12, 2.5, 3);
    g.fillPath(brush);
    g.rectPath(16.5, 12, 2.5, 3);
    g.fillPath(brush);
    // Bottom right corner tick mark
    g.moveTo(17, 18);
    g.lineTo(19, 18);
    g.lineTo(19, 16);
    g.lineTo(17, 18);
    g.fillPath(brush);
  });

  // Icon 4: Settings/Gear (Keyboard Shortcuts)
  var settingsBtn = createIconButton("Keyboard Shortcuts / Settings", function (g) {
    var brush = g.newBrush(g.BrushType.SOLID_COLOR, [0.72, 0.72, 0.72, 1.0]);
    var bg = g.newBrush(g.BrushType.SOLID_COLOR, [0.094, 0.094, 0.094, 1.0]);
    // Outer gear circular disk
    g.ellipsePath(3.5, 3.5, 15, 15);
    g.fillPath(brush);
    // 8 teeth (cardinals and diagonals)
    g.rectPath(9.5, 1.5, 3, 3); g.fillPath(brush); // top
    g.rectPath(9.5, 17.5, 3, 3); g.fillPath(brush); // bottom
    g.rectPath(1.5, 9.5, 3, 3); g.fillPath(brush); // left
    g.rectPath(17.5, 9.5, 3, 3); g.fillPath(brush); // right
    g.rectPath(4, 4, 3, 3); g.fillPath(brush); // top-left
    g.rectPath(15, 15, 3, 3); g.fillPath(brush); // bottom-right
    g.rectPath(15, 4, 3, 3); g.fillPath(brush); // top-right
    g.rectPath(4, 15, 3, 3); g.fillPath(brush); // bottom-left
    // Center hole cutout
    g.ellipsePath(8.5, 8.5, 5, 5);
    g.fillPath(bg);
  });

  // Icon 5: Action Logo (Cyan Video Copilot aperture logo - Apply Effect)
  var applyBtn = createIconButton("Apply selected effect to selected layers (Enter)", function (g) {
    var cyanBrush = g.newBrush(g.BrushType.SOLID_COLOR, [0.15, 0.72, 0.95, 1.0]); // Bright electric cyan #26b8f2
    var bg = g.newBrush(g.BrushType.SOLID_COLOR, [0.094, 0.094, 0.094, 1.0]);
    // 3 curved aperture blades forming the Video Copilot logo
    // Blade 1 (top-right)
    g.moveTo(11, 2.5);
    g.lineTo(19.5, 7.5);
    g.lineTo(15.5, 16);
    g.lineTo(11, 2.5);
    g.fillPath(cyanBrush);
    // Blade 2 (bottom)
    g.moveTo(19.5, 8.5);
    g.lineTo(11, 19.5);
    g.lineTo(4.5, 14.5);
    g.lineTo(19.5, 8.5);
    g.fillPath(cyanBrush);
    // Blade 3 (top-left)
    g.moveTo(10, 19.5);
    g.lineTo(2.5, 8.5);
    g.lineTo(10.5, 4);
    g.lineTo(10, 19.5);
    g.fillPath(cyanBrush);
    // Center aperture cutout opening
    g.ellipsePath(8, 8, 6, 6);
    g.fillPath(bg);
  });

  // ==================================================================
  // RESULTS CONTAINER (STATE B ONLY — Hidden in State A)
  // ==================================================================
  var resultsContainer = win.add("group");
  resultsContainer.orientation = "column";
  resultsContainer.alignChildren = ["fill", "top"];
  resultsContainer.spacing = 1;
  resultsContainer.margins = [0, 2, 0, 0];
  resultsContainer.visible = false;

  // Category Header Row
  var catRow = resultsContainer.add("group");
  catRow.orientation = "row";
  catRow.alignChildren = ["left", "center"];
  catRow.spacing = 6;
  catRow.margins = [16, 2, 4, 2];
  catRow.preferredSize = [TARGET_WIDTH - (WIN_MARGIN * 2), 22];
  catRow.onDraw = function () {
    var g = catRow.graphics;
    var rw = (catRow.size && catRow.size[0] > 0) ? catRow.size[0] : (TARGET_WIDTH - (WIN_MARGIN * 2));
    var rh = (catRow.size && catRow.size[1] > 0) ? catRow.size[1] : 22;
    var bg = g.newBrush(g.BrushType.SOLID_COLOR, [0.165, 0.165, 0.165, 1.0]);
    g.rectPath(0, 0, rw, rh);
    g.fillPath(bg);
  };

  // Small blue circular indicator
  var catDot = catRow.add("group");
  catDot.preferredSize = [6, 6];
  catDot.onDraw = function () {
    var g = catDot.graphics;
    var blue = g.newBrush(g.BrushType.SOLID_COLOR, [0.07, 0.45, 0.73, 1.0]); // #1272b8
    g.ellipsePath(0, 0, 6, 6);
    g.fillPath(blue);
  };

  // Category label ("Effects")
  var catTitle = catRow.add("statictext", undefined, "Effects");
  catTitle.alignment = ["left", "center"];
  try {
    catTitle.graphics.font = ScriptUI.newFont("Segoe UI", ScriptUI.FontStyle.REGULAR, 11);
    var catG = catTitle.graphics;
    catG.foregroundColor = catG.newBrush(catG.BrushType.SOLID_COLOR, [0.55, 0.57, 0.59, 1.0]);
  } catch (e) { }

  // Search Results List (Full Width, vibrant blue active selection)
  var resultsList = resultsContainer.add("listbox", undefined, []);
  resultsList.preferredSize = [TARGET_WIDTH - (WIN_MARGIN * 2), 62];
  resultsList.helpTip = "Matching effects. Click or use Up/Down keys to navigate.";
  try {
    resultsList.graphics.font = ScriptUI.newFont("Segoe UI", ScriptUI.FontStyle.REGULAR, 14);
  } catch (e) { }

  // Navigation and Close buttons (retained for programmatic navigation & Esc)
  var hiddenNav = win.add("group");
  hiddenNav.visible = false;
  var prevBtn = hiddenNav.add("button", undefined, "▲");
  var nextBtn = hiddenNav.add("button", undefined, "▼");
  var closeBtn = hiddenNav.add("button", undefined, "Esc");

  win.defaultElement = applyBtn;
  win.cancelElement = closeBtn;

  // ------------------------------------------------------------------
  // 7. Dynamic Layout & State Logic (Full-Width Responsive Adaptation)
  // ------------------------------------------------------------------
  var currentMatches = [];
  var isApplying = false;
  var currentListHeight = 58;
  var isLayoutInProgress = false;

  function getWindowWidth() {
    if (win.bounds && win.bounds.width > 0) return win.bounds.width;
    if (win.size && win.size[0] > 0) return win.size[0];
    if (win.preferredSize && win.preferredSize[0] > 0) return win.preferredSize[0];
    return TARGET_WIDTH;
  }

  function recalculateLayout(listH, explicitWidth) {
    if (isLayoutInProgress) return;
    isLayoutInProgress = true;

    try {
      if (typeof listH === "number") {
        currentListHeight = listH;
      }
      var winW = explicitWidth || getWindowWidth();
      var innerW = Math.max(260, winW - (WIN_MARGIN * 2));
      var topRowInnerW = Math.max(240, innerW - 4);

      // Calculate search field width to perfectly bridge search icon and toolbar
      var fixedW = SEARCH_ICON_W + ROW_SPACING + TOOLBAR_W + ROW_SPACING;
      var searchFieldW = Math.max(60, topRowInnerW - fixedW);

      searchField.characters = 0;
      searchField.preferredSize = [searchFieldW, 22];
      topRow.preferredSize = [innerW, 26];
      toolbar.preferredSize = [TOOLBAR_W, ICON_SIZE];

      catRow.preferredSize = [innerW, 22];
      resultsList.preferredSize = [innerW, currentListHeight];

      // Reference State B height: ~126px with 2 results (42 + 22 + 62 = 126)
      var totalH = resultsContainer.visible ? (INITIAL_HEIGHT + 22 + currentListHeight) : INITIAL_HEIGHT;
      win.preferredSize = [winW, totalH];
      win.layout.layout(true);
    } catch (e) {
      writeLn("FX Console: layout error: " + e.toString());
    } finally {
      isLayoutInProgress = false;
    }
  }

  // Hook resize handler: adapt dynamically when window size changes
  win.onResize = win.onResizing = function () {
    recalculateLayout();
  };

  function updateResults(query) {
    var trimmed = String(query === undefined || query === null ? "" : query).replace(/^\s+|\s+$/g, "");

    if (trimmed.length === 0) {
      // STATE A: Initial / Idle State (Toolbar only)
      resultsContainer.visible = false;
      currentMatches = [];
      resultsList.removeAll();
      recalculateLayout(24);
      return;
    }

    // STATE B: Active Search State
    resultsContainer.visible = true;
    currentMatches = filterEffects(trimmed);
    resultsList.removeAll();

    if (currentMatches.length === 0) {
      resultsList.add("item", "   No results found");
      resultsList.selection = null;
      applyBtn.enabled = false;

      var emptyH = 30;
      recalculateLayout(emptyH);
      return;
    }

    applyBtn.enabled = true;
    for (var i = 0; i < currentMatches.length; i++) {
      resultsList.add("item", "   " + currentMatches[i].name);
    }

    // Select the first matching result by default
    resultsList.selection = 0;

    // Dynamically adjust height to exact row count (clamped between 1 and 8 rows)
    var visibleRows = Math.min(currentMatches.length, 8);
    var rowHeight = 30;
    var listH = (visibleRows * rowHeight) + 2;
    recalculateLayout(listH);
  }

  function stepSelection(delta) {
    if (resultsContainer.visible && currentMatches.length > 0 && resultsList.items.length > 0) {
      var cur = resultsList.selection ? resultsList.selection.index : 0;
      var next = Math.max(0, Math.min(resultsList.items.length - 1, cur + delta));
      resultsList.selection = next;
    }
  }

  // ------------------------------------------------------------------
  // 8. Explicit & Safe Effect Application
  // ------------------------------------------------------------------
  function applySelectedEffect(source) {
    // STRICT GUARD: Effect application may ONLY occur via intentional user actions:
    // 1. Pressing Enter key ("enter")
    // 2. Clicking the dedicated Apply button ("apply_button")
    // 3. Programmatic test API ("api")
    // All other triggers (focus loss, clicks on X, toolbar, results, etc.) are strictly rejected.
    if (source !== "enter" && source !== "apply_button" && source !== "api") {
      return false;
    }

    if (isApplying) return false;

    var comp = getActiveComp();
    if (!comp) {
      writeLn("FX Console: No active composition found. Please open or select a comp.");
      return false;
    }

    var layers = getSelectedLayers(comp);
    if (layers.length === 0) {
      writeLn("FX Console: No layers selected. Please select at least one layer.");
      return false;
    }

    if (!resultsList.selection || currentMatches.length === 0) {
      return false;
    }

    var selIndex = resultsList.selection.index;
    if (selIndex < 0 || selIndex >= currentMatches.length) {
      return false;
    }

    var selectedEffect = currentMatches[selIndex];
    if (!selectedEffect || !selectedEffect.id) {
      return false;
    }

    isApplying = true;
    var undoStarted = false;
    var success = false;

    try {
      var layerIds = [];
      for (var l = 0; l < layers.length; l++) {
        if (layers[l] && layers[l].id !== undefined) {
          layerIds.push(layers[l].id);
        }
      }

      var undoGroupName = "Apply " + selectedEffect.name;
      app.beginUndoGroup(undoGroupName);
      undoStarted = true;

      var res = app.run("effect.apply", {
        comp: comp.id,
        effect: selectedEffect.id,
        layers: layerIds
      });

      if (res && (res.effects || res.applied !== false)) {
        success = true;
      }
    } catch (err) {
      writeLn("FX Console: Error applying effect: " + err.toString());
      success = false;
    } finally {
      if (undoStarted) {
        try {
          app.endUndoGroup();
        } catch (e) {
          writeLn("FX Console: Error ending undo group: " + e.toString());
        }
      }
      isApplying = false;
    }

    if (success) {
      try {
        win.close();
      } catch (e) { }
      return true;
    }

    return false;
  }

  // ------------------------------------------------------------------
  // 9. Event Handlers & Callbacks
  // ------------------------------------------------------------------
  // Live typing triggers dynamic state transition and filtering ONLY
  searchField.onChanging = function () {
    updateResults(searchField.text);
  };

  // Search field change handler ONLY updates/synchronizes filtering — NEVER applies effects!
  searchField.onChange = function () {
    updateResults(searchField.text);
  };

  // Result list selection change ONLY updates apply button state — NEVER applies effects!
  resultsList.onChange = function () {
    if (!resultsList.selection) return;
    applyBtn.enabled = true;
  };

  // Toolbar Callbacks (Each performs ONLY its dedicated task)
  cameraBtn.onClick = function () {
    try {
      app.run("view.takeSnapshot", {});
    } catch (e) {
      writeLn("FX Console: Snapshot failed: " + e.toString());
    }
  };

  galleryBtn.onClick = function () {
    try {
      app.run("view.showSnapshot", {});
    } catch (e) {
      writeLn("FX Console: Show snapshot failed: " + e.toString());
    }
  };

  copyBtn.onClick = function () {
    try {
      app.run("comp.copyFrame", {});
    } catch (e) {
      writeLn("FX Console: Copy frame failed: " + e.toString());
    }
  };

  settingsBtn.onClick = function () {
    try {
      app.run("app.keyboardShortcuts", {});
    } catch (e) {
      writeLn("FX Console: Open shortcuts failed: " + e.toString());
    }
  };

  // Dedicated Apply button: explicit intentional effect application
  applyBtn.onClick = function () {
    applySelectedEffect("apply_button");
  };

  prevBtn.onClick = function () {
    stepSelection(-1);
  };

  nextBtn.onClick = function () {
    stepSelection(1);
  };

  closeBtn.onClick = function () {
    win.close();
  };

  // Single Keyboard navigation listener attached strictly to window
  function handleKeyDown(ev) {
    if (!ev) return;
    var k = String(ev.keyName || ev.key || "");
    if (k === "Up" || k === "ArrowUp") {
      stepSelection(-1);
      if (typeof ev.preventDefault === "function") ev.preventDefault();
    } else if (k === "Down" || k === "ArrowDown") {
      stepSelection(1);
      if (typeof ev.preventDefault === "function") ev.preventDefault();
    } else if (k === "Enter" || k === "Return") {
      applySelectedEffect("enter");
      if (typeof ev.preventDefault === "function") ev.preventDefault();
    } else if (k === "Escape" || k === "Esc") {
      win.close();
      if (typeof ev.preventDefault === "function") ev.preventDefault();
    }
  }

  try {
    win.addEventListener("keydown", handleKeyDown);
    if (searchField) {
      searchField.addEventListener("keydown", handleKeyDown);
    }
  } catch (e) { }

  win.onClose = function () {
    try {
      win.removeEventListener("keydown", handleKeyDown);
      if (searchField) {
        searchField.removeEventListener("keydown", handleKeyDown);
      }
    } catch (e) { }
    globalScope.__fxConsoleInstance = null;
  };

  // ------------------------------------------------------------------
  // 10. Direct Focus via Engine ScriptUI Focus API
  // ------------------------------------------------------------------
  function requestSearchFocus() {
    if (searchField) {
      searchField.active = true;
    }
  }

  win.onShow = function () {
    requestSearchFocus();
  };

  // ------------------------------------------------------------------
  // 11. Initialization: Start in STATE A & Display
  // ------------------------------------------------------------------
  updateResults("");

  globalScope.__fxConsoleInstance = {
    win: win,
    searchField: searchField,
    resultsList: resultsList,
    resultsContainer: resultsContainer,
    applyBtn: applyBtn,
    prevBtn: prevBtn,
    nextBtn: nextBtn,
    cameraBtn: cameraBtn,
    galleryBtn: galleryBtn,
    copyBtn: copyBtn,
    settingsBtn: settingsBtn,
    recalculateLayout: recalculateLayout,
    activate: requestSearchFocus,
    requestFocus: requestSearchFocus,
    apply: function () {
      return applySelectedEffect("api");
    },
    update: updateResults,
    catalogue: cachedCatalogue
  };

  if (win instanceof Window) {
    win.center();
    win.show();
  } else if (win.layout) {
    win.layout.layout(true);
  }
  requestSearchFocus();
})(this);
