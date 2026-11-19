"use strict";

const $ = (selector) => document.querySelector(selector);
const element = (tag, text, className) => {
  const node = document.createElement(tag);
  if (text != null) node.textContent = text;
  if (className) node.className = className;
  return node;
};

const integer = new Intl.NumberFormat("fr-FR", {maximumFractionDigits: 0});
const decimal = new Intl.NumberFormat("fr-FR", {maximumFractionDigits: 2});
const meters = (mm) => mm > 0 && mm < 5 ? "< 0,01 m" : decimal.format(mm / 1000) + " m";
const count = (value) => integer.format(value);
const styles = getComputedStyle(document.documentElement);
const color = (name) => styles.getPropertyValue(name).trim();
const traceStyles = [
  ["wall", "--wall", 1.1, []],
  ["travel", "--travel", 0.85, []],
  ["cross", "--cross", 1.5, [4, 2]],
];

const slots = Object.fromEntries(
  [...document.querySelectorAll("[data-slot]")].map((node) => [
    node.dataset.slot, {node, reports: [], index: 0, busy: false, example: false},
  ]),
);
const state = {view: "reference", layer: 1, referenceTouched: false};
const visible = Object.fromEntries(traceStyles.map(([key]) => [key, true]));
const layerInput = $("#layer");
const map = $("#map");
const distribution = $("#distribution");

function duration(seconds) {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const rest = seconds % 60;
  return [hours ? hours + " h" : "", minutes ? minutes + " min" : "", rest || !seconds ? rest + " s" : ""]
    .filter(Boolean).join(" ");
}

function displayValue(value, format) {
  return value == null ? "—" : format(value);
}

function selected(slotName) {
  const slot = slots[slotName];
  return slot.reports[slot.index] || null;
}

function plateName(report) {
  const member = report.member?.split("/").pop();
  const match = member?.match(/^plate_(\d+)\.gcode$/i);
  return match ? "Plateau " + match[1] : member;
}

function settingsMap(report) {
  return new Map(report?.settings.map((item) => [item.key, item]));
}

function pairing() {
  const reference = selected("reference");
  const variant = selected("variant");
  if (!reference || !variant) return null;
  if (reference.digest === variant.digest) return {identical: true};

  const sameLayers = reference.layers.length === variant.layers.length &&
    reference.layers.every((layer, index) => {
      const z = variant.layers[index].z_mm;
      return layer.z_mm != null && z != null && Math.abs(layer.z_mm - z) < 0.005;
    });
  const conflicts = [];
  const checks = [
    ["modèle d’imprimante", reference.printer, variant.printer],
    ["diamètre de buse", reference.nozzle, variant.nozzle],
    ["version de Bambu Studio", reference.slicer, variant.slicer],
    ["profils de filament", reference.filament_profile, variant.filament_profile],
    ["filaments utilisés", reference.filaments.join(","), variant.filaments.join(",")],
    ["hauteur des objets", reference.objects.join(","), variant.objects.join(",")],
    ["séquence", settingsMap(reference).get("print_sequence")?.value, settingsMap(variant).get("print_sequence")?.value],
  ];
  for (const [label, a, b] of checks) {
    if (a && b && a !== b) conflicts.push(label);
  }
  if (!sameLayers) conflicts.push("plan des couches");
  return {reference, variant, sameLayers, conflicts};
}

function activeReport() {
  return selected(state.view) || selected("reference") || selected("variant");
}

function setSlotStatus(slot, message, error = false) {
  const status = slot.node.querySelector("[data-status]");
  status.textContent = message;
  status.dataset.error = String(error);
}

function renderSlot(slot) {
  const report = slot.reports[slot.index];
  const variant = slot.node.dataset.slot === "variant";
  const empty = variant ? "Ajoute un autre tranchage du même plateau" : "Choisis un export .gcode ou .gcode.3mf";
  slot.node.querySelector("[data-name]").textContent = report?.filename || empty;
  slot.node.querySelector("[data-context]").textContent = report ? [
    slot.example ? "Exemple chargé" : "",
    plateName(report),
    report.printer,
    report.layers.length + " couches",
  ].filter(Boolean).join(" · ") : "";
  const open = slot.node.querySelector("[data-open]");
  open.textContent = report ? "Remplacer" : "Importer";
  open.setAttribute("aria-label", (report ? "Remplacer " : "Importer ") +
    (variant ? "le tranchage de la variante" : "le tranchage de référence"));
  open.disabled = slot.busy;
  if (variant) {
    const clear = slot.node.querySelector("[data-clear]");
    clear.hidden = !report;
    clear.disabled = slot.busy;
  }
}

function setReports(slot, reports, example = false, file = null) {
  slot.reports = reports;
  slot.index = 0;
  slot.example = example;
  slot.file = file;
  const picker = slot.node.querySelector("[data-plate]");
  picker.replaceChildren(...reports.map((report, index) => {
    const option = element("option", plateName(report) || report.filename);
    option.value = index;
    return option;
  }));
  picker.value = "0";
  slot.node.querySelector("[data-plate-picker]").hidden = reports.length < 2;
}

async function importFile(slot, file) {
  if (!file.size || file.size > 64 * 1024 * 1024) {
    setSlotStatus(slot, "Choisis un export de 1 octet à 64 Mio.", true);
    return;
  }
  if (slot === slots.reference) state.referenceTouched = true;
  slot.busy = true;
  setSlotStatus(slot, "Analyse de l’export…");
  renderSlot(slot);
  try {
    const query = new URLSearchParams({nom: file.name});
    const response = await fetch("/analyse?" + query, {
      method: "POST",
      headers: {"Content-Type": "application/octet-stream"},
      body: file,
    });
    const data = await response.json().catch(() => null);
    if (!response.ok || !data) throw new Error(data?.error || "L’export n’a pas pu être analysé.");
    setReports(slot, data, false, file);
    setSlotStatus(slot, "");
    state.view = slot.node.dataset.slot;
    chooseBusyLayer();
  } catch (error) {
    setSlotStatus(slot, error.message || "L’analyse a échoué.", true);
  } finally {
    slot.busy = false;
    render();
  }
}

function renderMetrics(base, variant) {
  const metrics = [
    ["Temps total estimé", (report) => report.duration_s, duration],
    ["Distance XY à vide", (report) => report.totals.trajet_mm, meters],
    ["Segments croisant une paroi", (report) => report.totals.trajets_croisant_paroi, count],
    ["Séquences de rétraction", (report) => report.totals.retractations, count],
  ];
  $("#metrics").replaceChildren(...metrics.map(([label, read, format]) => {
    const metric = element("section", null, "metric");
    const value = read(base);
    metric.append(element("h2", label), element("p", displayValue(value, format), "metric-value"));
    if (!variant) return metric;
    const secondary = element("p", null, "metric-secondary");
    const next = read(variant);
    secondary.append(element("span", "Variante " + displayValue(next, format)));
    if (next != null && value != null) {
      const difference = next - value;
      const sign = difference < 0 ? "−" : "+";
      const flat = Math.abs(difference) < 0.000001;
      const delta = element("span", flat ? "0" : sign + format(Math.abs(difference)), "delta");
      delta.dataset.direction = flat ? "" : difference < 0 ? "down" : "up";
      delta.setAttribute("aria-label", "Écart variante moins référence " + delta.textContent);
      secondary.append(delta);
    }
    metric.append(secondary);
    return metric;
  }));
}

function settingValue(item) {
  if (!item) return "—";
  const booleans = ["reduce_crossing_wall", "avoid_crossing_wall_includes_support", "enable_support", "enable_prime_tower"];
  if (booleans.includes(item.key) && ["0", "1"].includes(item.value)) {
    return item.value === "1" ? "Activé" : "Désactivé";
  }
  const units = {travel_speed: "mm/s", travel_acceleration: "mm/s²", layer_height: "mm", initial_layer_print_height: "mm"};
  return units[item.key] ? item.value + " " + units[item.key] : item.value;
}

function renderSettings(base, variant) {
  const baseSettings = settingsMap(base);
  const variantSettings = settingsMap(variant);
  const keys = new Set([...baseSettings.keys(), ...variantSettings.keys()]);
  const rows = [];
  let changes = 0;
  for (const key of keys) {
    const a = baseSettings.get(key);
    const b = variantSettings.get(key);
    const row = element("tr");
    const label = element("th", (a || b).label);
    label.scope = "row";
    row.append(label, element("td", settingValue(a)));
    if (variant) {
      row.append(element("td", settingValue(b)));
      if (a && b && a.value !== b.value) {
        row.dataset.changed = "true";
        changes += 1;
      }
    }
    rows.push(row);
  }
  $("#settings-rows").replaceChildren(...rows);
  const hasReference = Boolean(selected("reference"));
  $("#settings table th:nth-child(2)").textContent = hasReference ? "Référence" : "Variante";
  $("#variant-head").hidden = !variant;
  $("#settings").hidden = !rows.length;
  $("#settings-note").textContent = variant
    ? count(changes) + " réglage" + (changes > 1 ? "s" : "") + " différent" + (changes > 1 ? "s" : "") + " parmi les valeurs présentes dans les deux exports."
    : "Valeurs relevées dans l’export sélectionné.";
}

function renderTest(base, pair) {
  const avoid = settingsMap(base).get("reduce_crossing_wall")?.value;
  let instruction;
  if (pair?.conflicts?.length) {
    instruction = "Retranche le même plateau avec le même profil d’imprimante et le même filament pour isoler un seul changement.";
  } else if (pair && !pair.identical) {
    const variant = pair.variant;
    const variantSettings = settingsMap(variant);
    const crossesLess = variant.totals.trajets_croisant_paroi < base.totals.trajets_croisant_paroi;
    const takesLonger = variant.duration_s != null && base.duration_s != null && variant.duration_s > base.duration_s;
    if (crossesLess && takesLonger && variantSettings.get("reduce_crossing_wall")?.value === "1" &&
        variantSettings.get("max_travel_detour_distance")?.value === "0") {
      instruction = "Pour tester un détour plus court, duplique la variante et essaie une longueur maximale de détour de 50 %. Compare le nouvel export à la même référence.";
    } else {
      instruction = "Vérifie les couches où les trajets diffèrent dans l’aperçu de Bambu Studio. Compare ensuite la durée réelle et l’état des parois sur la pièce imprimée.";
    }
  } else if (avoid === "0" && base.totals.trajets_croisant_paroi > 0) {
    instruction = "Dans le processus avancé de Bambu Studio, ouvre Qualité → Avancé et active seulement « Éviter de traverser les parois » (Avoid crossing wall). Retranche le même plateau et importe le nouvel export en variante.";
  } else {
    instruction = "Retranche le même plateau après avoir changé un seul réglage de processus. Importe le nouvel export en variante pour mesurer l’écart.";
  }
  $("#test-instruction").textContent = instruction;
}

function chooseBusyLayer() {
  const report = activeReport();
  state.layer = report?.layers.reduce((best, item) => item.trajet_mm > best.trajet_mm ? item : best).couche || 1;
}

function setLayer(value) {
  const report = activeReport();
  if (!report) return;
  state.layer = Math.min(report.layers.length, Math.max(1, value));
  renderLayer();
}

function renderHotspots(report) {
  const list = $("#hotspots");
  const busy = [...report.layers].sort((a, b) => b.trajet_mm - a.trajet_mm).filter((item) => item.trajet_mm > 0).slice(0, 5);
  if (!busy.length) {
    list.replaceChildren(element("p", "Aucun trajet mesuré.", "fine"));
    return;
  }
  list.replaceChildren(...busy.map((layer) => {
    const button = element("button", null, "hotspot");
    button.type = "button";
    button.dataset.layer = layer.couche;
    button.append(element("span", "Couche " + layer.couche), element("span", meters(layer.trajet_mm)));
    button.addEventListener("click", () => setLayer(layer.couche));
    return button;
  }));
}

function canvasContext(canvas) {
  const bounds = canvas.getBoundingClientRect();
  if (!bounds.width || !bounds.height) return null;
  const ratio = window.devicePixelRatio || 1;
  const width = bounds.width;
  const height = bounds.height;
  const bufferWidth = Math.round(width * ratio);
  const bufferHeight = Math.round(height * ratio);
  if (canvas.width !== bufferWidth || canvas.height !== bufferHeight) {
    canvas.width = bufferWidth;
    canvas.height = bufferHeight;
  }
  const ctx = canvas.getContext("2d");
  ctx.setTransform(bufferWidth / width, 0, 0, bufferHeight / height, 0, 0);
  ctx.clearRect(0, 0, width, height);
  return {ctx, width, height};
}

function gridStep(target) {
  const base = 10 ** Math.floor(Math.log10(target));
  return [1, 2, 5, 10].find((step) => step * base >= target) * base;
}

function drawMap(layer, other) {
  const drawing = canvasContext(map);
  if (!drawing) return;
  const {ctx, width, height} = drawing;
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const current of [layer, other].filter(Boolean)) {
    for (const values of Object.values(current.trace)) {
      for (let index = 0; index < values.length; index += 2) {
        const x = values[index];
        const y = values[index + 1];
        minX = Math.min(minX, x);
        minY = Math.min(minY, y);
        maxX = Math.max(maxX, x);
        maxY = Math.max(maxY, y);
      }
    }
  }
  const hasTrace = Object.values(layer.trace).some((values) => values.length);
  ctx.font = "11px system-ui";
  ctx.fillStyle = color("--map-axis");
  if (!hasTrace) {
    ctx.textAlign = "center";
    ctx.fillText("Aucun tracé sur cette couche", width / 2, height / 2);
    return;
  }

  const left = 43, right = width - 17, top = 18, bottom = height - 34;
  const midX = (minX + maxX) / 2, midY = (minY + maxY) / 2;
  const centerX = (left + right) / 2, centerY = (top + bottom) / 2;
  const scale = 0.88 * Math.min((right - left) / Math.max(maxX - minX, 8), (bottom - top) / Math.max(maxY - minY, 8));
  const px = (x) => centerX + (x - midX) * scale;
  const py = (y) => centerY - (y - midY) * scale;
  const step = gridStep(55 / scale);
  const x0 = Math.ceil((midX + (left - centerX) / scale) / step);
  const y0 = Math.ceil((midY - (bottom - centerY) / scale) / step);
  ctx.strokeStyle = color("--map-grid");
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.textAlign = "center";
  for (let i = x0; px(i * step) <= right; i++) {
    const x = px(i * step);
    ctx.moveTo(x, top);
    ctx.lineTo(x, bottom);
    ctx.fillText(decimal.format(i * step), x, bottom + 16);
  }
  ctx.textAlign = "right";
  for (let i = y0; py(i * step) >= top; i++) {
    const y = py(i * step);
    ctx.moveTo(left, y);
    ctx.lineTo(right, y);
    ctx.fillText(decimal.format(i * step), left - 6, y + 4);
  }
  ctx.stroke();
  ctx.fillText("X mm", right, height - 5);
  ctx.textAlign = "left";
  ctx.fillText("Y mm", 8, 14);

  for (const [key, tone, lineWidth, dash] of traceStyles) {
    if (!visible[key]) continue;
    const values = layer.trace[key];
    ctx.beginPath();
    ctx.setLineDash(dash);
    ctx.strokeStyle = color(tone);
    ctx.lineWidth = lineWidth;
    for (let i = 0; i < values.length; i += 4) {
      ctx.moveTo(px(values[i]), py(values[i + 1]));
      ctx.lineTo(px(values[i + 2]), py(values[i + 3]));
    }
    ctx.stroke();
  }
  ctx.setLineDash([]);
}

function drawDistribution(series) {
  const drawing = canvasContext(distribution);
  if (!drawing) return;
  const {ctx, width, height} = drawing;
  const left = 30, right = width - 12, top = 12, bottom = height - 23;
  const first = series[0].report.layers;
  const peak = Math.max(1, ...series.map(({report}) => report.layers.reduce((max, {trajet_mm}) => Math.max(max, trajet_mm), 0)));
  const step = gridStep(peak / 2);
  const ceiling = Math.ceil(peak / step) * step;
  const band = (right - left) / first.length;
  const x = (index) => left + (index + 0.5) * band;
  const y = (mm) => bottom - mm / ceiling * (bottom - top);
  ctx.font = "11px system-ui";
  ctx.fillStyle = color("--muted");
  ctx.strokeStyle = color("--grid");
  ctx.textAlign = "right";
  ctx.beginPath();
  for (let mm = 0; mm <= ceiling; mm += step) {
    ctx.moveTo(left, y(mm));
    ctx.lineTo(right, y(mm));
    ctx.fillText(decimal.format(mm / 1000), left - 4, y(mm) + 3);
  }
  ctx.stroke();
  for (const {name, report} of series) {
    if (name === "reference" || series.length === 1) {
      ctx.fillStyle = color(name === "reference" ? "--accent" : "--variant");
      for (const [index, layer] of report.layers.entries()) {
        const barWidth = Math.max(0.6, Math.min(16, band * 0.78));
        ctx.fillRect(x(index) - barWidth / 2, y(layer.trajet_mm), barWidth, bottom - y(layer.trajet_mm));
      }
    } else {
      ctx.beginPath();
      ctx.strokeStyle = color("--variant");
      ctx.setLineDash([4, 2]);
      ctx.lineWidth = 1.25;
      report.layers.forEach(({trajet_mm}, index) => ctx.lineTo(x(index), y(trajet_mm)));
      ctx.stroke();
      ctx.setLineDash([]);
    }
  }
  ctx.strokeStyle = color("--ink");
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(x(state.layer - 1), top);
  ctx.lineTo(x(state.layer - 1), bottom);
  ctx.stroke();
  ctx.fillStyle = color("--muted");
  ctx.textAlign = "left";
  ctx.fillText("Couche 1", left, bottom + 16);
  if (first.length > 1) {
    ctx.textAlign = "right";
    ctx.fillText(count(first.length), right, bottom + 16);
  }
}

function renderLayer() {
  const report = activeReport();
  if (!report) return;
  const layer = report.layers[state.layer - 1];
  const pair = pairing();
  const aligned = pair && !pair.identical && pair.sameLayers;
  const other = aligned ? (state.view === "reference" ? pair.variant : pair.reference) : null;
  const layerNumber = layer.couche + " / " + report.layers.length;
  const height = layer.z_mm == null ? "" : " · Z " + decimal.format(layer.z_mm) + " mm";
  layerInput.value = layer.couche;
  layerInput.setAttribute("aria-valuetext", "Couche " + layerNumber + height);
  $("#layer-number").textContent = layerNumber;
  $("#previous").disabled = layer.couche === 1;
  $("#next").disabled = layer.couche === report.layers.length;
  $("#layer-caption").textContent = [
    "Couche " + layer.couche + height,
    meters(layer.trajet_mm) + " à vide",
    count(layer.trajets_croisant_paroi) + (layer.trajets_croisant_paroi === 1 ? " croisement" : " croisements"),
  ].join(" · ");
  for (const button of $("#hotspots").querySelectorAll("button")) {
    button.setAttribute("aria-current", String(Number(button.dataset.layer) === layer.couche));
  }
  map.setAttribute("aria-label", "Trajectoires XY. " + $("#layer-caption").textContent);
  drawMap(layer, other?.layers[state.layer - 1]);

  const series = aligned
    ? [{name: "reference", report: pair.reference}, {name: "variant", report: pair.variant}]
    : [{name: state.view, report}];
  drawDistribution(series);
}

function render() {
  Object.values(slots).forEach(renderSlot);
  document.dispatchEvent(new Event("atelier:exports"));
  const reference = selected("reference");
  const variant = selected("variant");
  const base = reference || variant;
  const pair = pairing();
  const comparing = pair && !pair.identical;
  $("#export").disabled = !base;
  $("#results").hidden = !base;
  if (base) $("#starting").hidden = true;
  if (!base) return;
  if (!comparing) state.view = reference ? "reference" : "variant";
  const report = activeReport();
  state.layer = Math.min(state.layer, report.layers.length);
  layerInput.max = report.layers.length;
  layerInput.disabled = report.layers.length === 1;

  const status = $("#pair-status");
  status.hidden = !pair;
  if (pair?.identical) {
    status.dataset.kind = "";
    status.textContent = "Même G-code dans les deux exports. Importe un nouveau tranchage pour mesurer un écart.";
  } else if (pair?.conflicts.length) {
    status.dataset.kind = "caution";
    status.textContent = "Les exports diffèrent sur les points suivants — " + pair.conflicts.join(", ") + ".";
  } else if (pair) {
    status.dataset.kind = "";
    status.textContent = "Les hauteurs de couche concordent. Pour isoler un réglage, utilise le même projet et la même disposition.";
  }

  $("#view-picker").hidden = !comparing;
  for (const button of $("#view-picker").querySelectorAll("button")) {
    button.setAttribute("aria-pressed", String(button.dataset.view === state.view));
  }
  const distributionLegend = $("#distribution-legend");
  distributionLegend.replaceChildren();
  if (comparing && pair.sameLayers) {
    for (const [name, caption] of [["reference", "Référence"], ["variant", "Variante"]]) {
      const legend = element("span");
      legend.append(element("i", null, "swatch " + name), element("span", caption));
      distributionLegend.append(legend);
    }
  }
  renderMetrics(base, comparing ? variant : null);
  renderSettings(base, comparing ? variant : null);
  renderTest(base, pair);
  renderHotspots(report);
  renderLayer();
}

function downloadCsv() {
  const reference = selected("reference");
  const variant = selected("variant");
  const fields = [
    "couche", "z_mm", "trajets_xy", "trajet_mm", "tour_purge_mm", "retractations",
    "remontees_z", "arcs", "trajets_croisant_paroi", "changements_filament",
  ];
  const lines = [["tranchage", ...fields].join(";")];
  for (const [name, report] of [["reference", reference], ["variante", variant]]) {
    if (!report || name === "variante" && report.digest === reference?.digest) continue;
    for (const layer of report.layers) {
      lines.push([name, ...fields.map((key) => String(layer[key] ?? "").replace(".", ","))].join(";"));
    }
  }
  const url = URL.createObjectURL(new Blob(["\uFEFF", lines.join("\r\n"), "\r\n"], {type: "text/csv;charset=utf-8"}));
  const link = element("a");
  link.href = url;
  link.download = "atelier-gcode-mesures.csv";
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

for (const [name, slot] of Object.entries(slots)) {
  const fileInput = slot.node.querySelector("[data-file]");
  slot.node.querySelector("[data-open]").addEventListener("click", () => fileInput.click());
  fileInput.addEventListener("change", () => {
    const file = fileInput.files[0];
    fileInput.value = "";
    if (file) importFile(slot, file);
  });
  slot.node.querySelector("[data-plate]").addEventListener("change", (event) => {
    slot.index = Number(event.target.value);
    state.view = name;
    chooseBusyLayer();
    render();
  });
}
slots.variant.node.querySelector("[data-clear]").addEventListener("click", () => {
  setReports(slots.variant, []);
  setSlotStatus(slots.variant, "");
  state.view = "reference";
  render();
});
for (const button of $("#view-picker").querySelectorAll("button")) {
  button.addEventListener("click", () => {
    state.view = button.dataset.view;
    render();
  });
}
for (const input of $("#legend").querySelectorAll("input")) {
  input.addEventListener("change", () => {
    visible[input.dataset.trace] = input.checked;
    renderLayer();
  });
}
layerInput.addEventListener("input", () => setLayer(Number(layerInput.value)));
$("#previous").addEventListener("click", () => setLayer(state.layer - 1));
$("#next").addEventListener("click", () => setLayer(state.layer + 1));
$("#export").addEventListener("click", downloadCsv);
const plots = new ResizeObserver(() => renderLayer());
plots.observe(map);
plots.observe(distribution);

fetch("/exemple.json")
  .then((response) => response.ok ? response.json() : null)
  .then((report) => {
    if (report && !state.referenceTouched) {
      setReports(slots.reference, [report], true);
      state.view = "reference";
      chooseBusyLayer();
      render();
    }
  })
  .catch(() => {})
  .finally(() => { $("#starting").hidden = true; });

async function showVersion() {
  try {
    const response = await fetch("/version", { cache: "no-store" });
    if (!response.ok) return;
    const info = await response.json();
    const label = [`Atelier G-code ${info.version}`, info.message].filter(Boolean).join(" · ");
    const footer = $("#app-version");
    if (footer.textContent !== label) footer.textContent = label;
    if (info.checking) window.setTimeout(showVersion, 2500);
  } catch {
    // The local application may have been closed while the browser tab stayed open.
  }
}
showVersion();
