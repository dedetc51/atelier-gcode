"use strict";

(() => {
  const form = document.querySelector("#print-form");
  const source = document.querySelector("#print-source");
  const sourceNote = document.querySelector("#print-source-note");
  const fields = document.querySelector("#printer-fields");
  const connect = document.querySelector("#print-connect");
  const mapping = document.querySelector("#print-filaments");
  const confirmation = document.querySelector("#print-confirm");
  const submit = document.querySelector("#print-submit");
  const message = document.querySelector("#print-status");
  const credentials = ["ip", "serial", "access_code"].map((name) => form.elements.namedItem(name));
  let candidates = [];
  let current = null;
  let device = null;
  let busy = false;

  function status(text, error = false) {
    message.textContent = text;
    message.dataset.error = String(error);
  }

  function ready() {
    return device && current && ["IDLE", "FINISH"].includes(device.state) &&
      (device.nozzle == null || Math.abs(device.nozzle - current.report.print_options.nozzle) < 0.01);
  }

  function actions() {
    fields.disabled = !current || busy;
    source.disabled = candidates.length === 0 || busy;
    const trays = [...mapping.querySelectorAll("select")];
    trays.forEach((tray) => { tray.disabled = busy; });
    confirmation.disabled = busy;
    submit.disabled = busy || !ready() || !confirmation.checked || !trays.length ||
      trays.some((tray) => !tray.value) || new Set(trays.map((tray) => tray.value)).size !== trays.length;
  }

  function options() {
    confirmation.checked = false;
    mapping.replaceChildren();
    const show = Boolean(device && current);
    mapping.hidden = !show;
    document.querySelector("#print-confirm-label").hidden = !show;
    if (!show) {
      actions();
      return;
    }
    const filaments = current.report.print_options.filaments;
    for (const filament of filaments) {
      const label = element("label");
      const detail = [filament.material, filament.color].filter(Boolean).join(" · ");
      label.append(element("span", "Filament " + (filament.index + 1) + " · " + detail));
      const select = element("select");
      select.required = true;
      const empty = element("option", "Choisir un emplacement");
      empty.value = "";
      select.append(empty);
      for (const tray of device.trays) {
        if (tray.id === 254 && filaments.length > 1) continue;
        if (tray.material && tray.material.toLowerCase() !== filament.material.toLowerCase()) continue;
        const item = element("option", [tray.label, tray.material, tray.color].filter(Boolean).join(" · "));
        item.value = tray.id;
        select.append(item);
      }
      if (select.options.length === 1) empty.textContent = "Aucun emplacement compatible annoncé";
      select.addEventListener("change", () => {
        confirmation.checked = false;
        actions();
      });
      label.append(select);
      mapping.append(label);
    }
    actions();
  }

  function changeSource() {
    current = candidates.find((candidate) => candidate.key === source.value) || null;
    if (current) {
      const {nozzle, bed_type: bed} = current.report.print_options;
      sourceNote.textContent = "Buse " + decimal.format(nozzle) + " mm · " + bed.replaceAll("_", " ");
    } else {
      const printError = ["reference", "variant"].map(selected).find((report) => report?.print_error)?.print_error;
      sourceNote.textContent = printError ||
        "Dans Bambu Studio, exporte le plateau tranché en .gcode.3mf, puis importe-le comme référence ou variante.";
    }
    options();
  }

  function refreshSources() {
    const previous = current;
    candidates = Object.entries(slots).flatMap(([name, slot]) => {
      const report = selected(name);
      return slot.file && report?.print_options
        ? [{key: name + "/" + report.digest + "/" + report.member, name, file: slot.file, report}]
        : [];
    });
    const unchanged = previous && candidates.find((candidate) =>
      candidate.key === previous.key && candidate.file === previous.file);
    if (!candidates.length) {
      source.replaceChildren(element("option", "Aucun export imprimable importé"));
      source.value = "";
    } else {
      source.replaceChildren(...candidates.map(({key, name, report}) => {
        const label = name === "reference" ? "Référence" : "Variante";
        const item = element("option", [label, report.filename, plateName(report)].filter(Boolean).join(" · "));
        item.value = key;
        return item;
      }));
      source.value = unchanged?.key || candidates[0].key;
    }
    if (unchanged) {
      current = unchanged;
      actions();
    } else {
      changeSource();
    }
  }

  function printerSettings() {
    const settings = Object.fromEntries(credentials.map((input) => [input.name, input.value.trim()]));
    settings.serial = settings.serial.toUpperCase();
    return settings;
  }

  async function request(path, init) {
    const response = await fetch(path, init);
    const data = await response.json().catch(() => null);
    if (!response.ok || !data) throw new Error(data?.error || "La demande a échoué.");
    return data;
  }

  connect.addEventListener("click", async () => {
    if (busy || !credentials.every((input) => input.reportValidity())) return;
    busy = true;
    device = null;
    options();
    status("Lecture de l’état de la P2S…");
    try {
      device = await request("/connexion", {
        method: "POST",
        headers: {"Content-Type": "application/json"},
        body: JSON.stringify(printerSettings()),
      });
      const nozzle = device.nozzle == null ? "" : " · buse " + decimal.format(device.nozzle) + " mm";
      status("P2S connectée · état " + device.state + nozzle, !ready());
    } catch (error) {
      status(error.message, true);
    } finally {
      busy = false;
      options();
    }
  });

  credentials.forEach((input) => input.addEventListener("input", () => {
    device = null;
    status("");
    options();
  }));

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (submit.disabled || !current) return;
    const candidate = current;
    const parameters = {
      printer: printerSettings(),
      member: candidate.report.member,
      digest: candidate.report.digest,
      trays: [...mapping.querySelectorAll("select")].map((input) => Number(input.value)),
      confirmed: confirmation.checked,
    };
    busy = true;
    confirmation.checked = false;
    actions();
    status("Vérification de la P2S et transfert du plateau…");
    try {
      const result = await request("/imprimer", {
        method: "POST",
        headers: {"Content-Type": "application/octet-stream", "X-Atelier-Print": JSON.stringify(parameters)},
        body: candidate.file,
      });
      if (result.state === "started") {
        status("Démarrage signalé par la P2S · " + result.job);
      } else if (result.state === "accepted") {
        status("Commande acceptée par la P2S. Suis le démarrage sur son écran · " + result.job);
      } else {
        status("Démarrage à vérifier sur l’écran de la P2S avant tout nouvel envoi · " + result.job, true);
      }
    } catch (error) {
      status(error.message + " Vérifie l’écran de la P2S avant un nouvel envoi.", true);
    } finally {
      busy = false;
      device = null;
      options();
    }
  });

  source.addEventListener("change", changeSource);
  confirmation.addEventListener("change", actions);
  document.addEventListener("atelier:exports", refreshSources);
  refreshSources();
})();
