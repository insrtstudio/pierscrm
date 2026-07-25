import { useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";
import { FileSpreadsheet, Upload, ArrowRight, Check } from "lucide-react";
import { importFile, previewFile } from "../lib/api";
import type { FilePreview } from "../lib/types";
import { CATEGORIES, IMPORT_FIELDS } from "../lib/constants";
import { PageHeader } from "../components/Layout";
import { Field, useToast } from "../components/ui";

/** Guess a field for a given header label. */
function guessField(header: string): string {
  const h = header.toLowerCase().trim();
  const rules: [string, string][] = [
    ["name", "name"],
    ["event", "name"],
    ["venue", "venue"],
    ["promoter", "promoter"],
    ["collective", "promoter"],
    ["priority", "priority"],
    ["fit", "priority"],
    ["type", "type"],
    ["area", "area"],
    ["scale", "scale"],
    ["capacity", "scale"],
    ["date", "date"],
    ["time", "time"],
    ["format", "format"],
    ["why", "reason"],
    ["description", "reason"],
    ["reason", "reason"],
    ["channel", "contact_channel"],
    ["email status", "email_status"],
    ["email", "email"],
    ["status", "status"],
    ["first contact", "first_contact"],
    ["follow", "follow_up"],
    ["notes", "notes"],
    ["website", "website"],
    ["link", "website"],
    ["tags", "tags"],
  ];
  for (const [needle, field] of rules) {
    if (h.includes(needle)) return field;
  }
  return "";
}

export function Import() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();

  const [path, setPath] = useState<string | null>(null);
  const [preview, setPreview] = useState<FilePreview | null>(null);
  const [sheetIdx, setSheetIdx] = useState(0);
  const [category, setCategory] = useState("venue");
  const [mapping, setMapping] = useState<Record<number, string>>({});
  const [busy, setBusy] = useState(false);

  const sheet = preview?.sheets[sheetIdx];

  const pickFile = async () => {
    const selected = await open({
      multiple: false,
      filters: [
        { name: "Data", extensions: ["xlsx", "xls", "xlsm", "csv", "tsv"] },
      ],
    });
    if (!selected || typeof selected !== "string") return;
    setPath(selected);
    try {
      const p = await previewFile(selected);
      setPreview(p);
      setSheetIdx(0);
      applyGuess(p, 0);
    } catch (e: any) {
      toast(e?.toString?.() ?? "error", "error");
    }
  };

  const applyGuess = (p: FilePreview, idx: number) => {
    const s = p.sheets[idx];
    const m: Record<number, string> = {};
    s?.headers.forEach((h, i) => {
      const f = guessField(h);
      if (f) m[i] = f;
    });
    setMapping(m);
  };

  const changeSheet = (idx: number) => {
    setSheetIdx(idx);
    if (preview) applyGuess(preview, idx);
  };

  const hasName = useMemo(
    () => Object.values(mapping).includes("name"),
    [mapping]
  );

  // Fields already used, to prevent duplicate assignment.
  const usedFields = new Set(Object.values(mapping).filter(Boolean));

  const run = async () => {
    if (!path || !sheet || !hasName) return;
    setBusy(true);
    // Build field -> column-index map (invert), keeping first occurrence.
    const fieldMap: Record<string, number> = {};
    for (const [col, field] of Object.entries(mapping)) {
      if (field && !(field in fieldMap)) fieldMap[field] = Number(col);
    }
    try {
      const res = await importFile({
        path,
        sheet: preview?.kind === "xlsx" ? sheet.name : undefined,
        category,
        mapping: fieldMap,
        skip_rows: sheet.header_row + 1,
      });
      toast(
        t("import.done", { inserted: res.inserted, skipped: res.skipped }),
        "ok"
      );
      qc.invalidateQueries({ queryKey: ["contacts"] });
      qc.invalidateQueries({ queryKey: ["dashboard"] });
      // reset
      setPath(null);
      setPreview(null);
      setMapping({});
    } catch (e: any) {
      toast(e?.toString?.() ?? "error", "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div>
      <PageHeader title={t("import.title")} subtitle={t("import.subtitle")} />
      <div className="px-8 pb-10">
        {!preview ? (
          <button
            onClick={pickFile}
            className="card flex w-full flex-col items-center justify-center gap-3 border-2 border-dashed border-border py-20 text-center transition-colors hover:border-accent hover:bg-muted/40"
          >
            <div className="flex h-14 w-14 items-center justify-center rounded-2xl bg-accent/10 text-accent">
              <Upload size={24} />
            </div>
            <div className="text-sm font-medium">{t("import.drop")}</div>
            <div className="text-xs text-fg-subtle">{t("import.supported")}</div>
          </button>
        ) : (
          <div className="space-y-5">
            {/* File header */}
            <div className="card flex items-center gap-3 p-4">
              <FileSpreadsheet size={20} className="text-accent" />
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm font-medium">
                  {path?.split("/").pop()}
                </div>
                <div className="text-xs text-fg-subtle">
                  {t("import.rows_detected", { count: sheet?.total_rows ?? 0 })}
                </div>
              </div>
              <button className="btn-outline" onClick={pickFile}>
                {t("import.pick")}
              </button>
            </div>

            {/* Controls */}
            <div className="grid grid-cols-2 gap-4">
              {preview.kind === "xlsx" && preview.sheets.length > 1 && (
                <Field label={t("import.sheet")}>
                  <select
                    className="input"
                    value={sheetIdx}
                    onChange={(e) => changeSheet(Number(e.target.value))}
                  >
                    {preview.sheets.map((s, i) => (
                      <option key={i} value={i}>
                        {s.name} ({s.total_rows})
                      </option>
                    ))}
                  </select>
                </Field>
              )}
              <Field label={t("import.category")}>
                <select
                  className="input"
                  value={category}
                  onChange={(e) => setCategory(e.target.value)}
                >
                  {CATEGORIES.map((c) => (
                    <option key={c} value={c}>
                      {t(`category.${c}`)}
                    </option>
                  ))}
                </select>
              </Field>
            </div>

            {/* Mapping */}
            <div className="card p-5">
              <h3 className="text-sm font-semibold">{t("import.mapping")}</h3>
              <p className="mt-0.5 text-xs text-fg-subtle">
                {t("import.mapping_help")}
              </p>
              <div className="mt-4 grid grid-cols-1 gap-2 md:grid-cols-2">
                {sheet?.headers.map((h, i) => (
                  <div
                    key={i}
                    className="flex items-center gap-2 rounded-lg border border-border px-3 py-2"
                  >
                    <span className="min-w-0 flex-1 truncate text-sm" title={h}>
                      {h}
                    </span>
                    <ArrowRight size={14} className="shrink-0 text-fg-subtle" />
                    <select
                      className="input w-40 py-1.5 text-xs"
                      value={mapping[i] ?? ""}
                      onChange={(e) =>
                        setMapping((m) => ({ ...m, [i]: e.target.value }))
                      }
                    >
                      <option value="">{t("import.ignore")}</option>
                      {IMPORT_FIELDS.map((f) => (
                        <option
                          key={f}
                          value={f}
                          disabled={usedFields.has(f) && mapping[i] !== f}
                        >
                          {t(`contacts.${f}`, f)}
                        </option>
                      ))}
                    </select>
                  </div>
                ))}
              </div>
            </div>

            {/* Preview table */}
            <div className="card overflow-hidden">
              <div className="border-b border-border px-4 py-2.5 text-xs font-medium text-fg-subtle">
                {t("import.preview")}
              </div>
              <div className="overflow-x-auto">
                <table className="w-full text-xs">
                  <thead>
                    <tr className="border-b border-border text-left text-fg-subtle">
                      {sheet?.headers.map((h, i) => (
                        <th key={i} className="whitespace-nowrap px-3 py-2 font-medium">
                          {mapping[i] ? (
                            <span className="text-accent">
                              {t(`contacts.${mapping[i]}`, mapping[i])}
                            </span>
                          ) : (
                            h
                          )}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {sheet?.rows.map((row, ri) => (
                      <tr key={ri} className="border-b border-border/50">
                        {sheet.headers.map((_, ci) => (
                          <td
                            key={ci}
                            className="max-w-[220px] truncate px-3 py-1.5"
                            title={row[ci] ?? ""}
                          >
                            {row[ci] ?? ""}
                          </td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>

            {/* Run */}
            <div className="flex items-center justify-end gap-3">
              {!hasName && (
                <span className="text-xs text-amber-500">
                  {t("import.need_name")}
                </span>
              )}
              <button
                className="btn-primary"
                disabled={!hasName || busy}
                onClick={run}
              >
                <Check size={16} />
                {busy ? t("import.importing") : t("import.run")}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
