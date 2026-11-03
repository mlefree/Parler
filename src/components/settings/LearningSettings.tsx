import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { commands, type LearningSample } from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import {
  Dropdown,
  SettingsGroup,
  Textarea,
  ToggleSwitch,
} from "@/components/ui";
import { Button } from "@/components/ui/Button";
import { Input } from "@/components/ui/Input";

export const LearningSettings: React.FC = () => {
  const { t } = useTranslation();
  const { settings, refreshSettings } = useSettings();
  const enabled = settings?.learning?.enabled ?? false;
  const audio = settings?.learning?.save_audio ?? true;
  const savedDirectory = settings?.learning?.directory ?? "";
  const [directory, setDirectory] = useState(savedDirectory);
  const [samples, setSamples] = useState<LearningSample[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [correction, setCorrection] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const selected = samples.find((sample) => sample.id === selectedId);

  useEffect(() => setDirectory(savedDirectory), [savedDirectory]);
  useEffect(() => {
    setCorrection(selected?.user_correction ?? "");
  }, [selected?.id, selected?.user_correction]);

  useEffect(() => setSaved(false), [selected?.id]);

  const refresh = useCallback(async () => {
    try {
      const result = await commands.getLearningSamples();
      if (result.status === "error") throw new Error(result.error);
      setSamples(result.data);
      if (!result.data.some((sample) => sample.id === selectedId))
        setSelectedId(result.data[0]?.id ?? null);
    } catch (error) {
      setError(String(error));
    }
  }, [selectedId]);

  useEffect(() => {
    void refresh();
    const unlisten = listen("learning-sample-added", () => {
      void refresh();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refresh, savedDirectory]);

  const configure = async (nextEnabled: boolean, nextAudio: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const result = await commands.configureLearning(
        nextEnabled,
        nextAudio,
        directory.trim(),
      );
      if (result.status === "error") throw new Error(result.error);
      await refreshSettings();
      await refresh();
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(false);
    }
  };

  const saveCorrection = async () => {
    if (!selectedId) return;
    setBusy(true);
    setError(null);
    try {
      const result = await commands.saveLearningCorrection(
        selectedId,
        correction,
      );
      if (result.status === "error") throw new Error(result.error);
      await refresh();
      setSaved(true);
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsGroup title={t("settings.learning.title")}>
      <ToggleSwitch
        checked={enabled}
        onChange={(value) => void configure(value, audio)}
        isUpdating={busy}
        label={t("settings.learning.enabled")}
        description={t("settings.learning.description")}
        descriptionMode="inline"
        grouped
      />
      <ToggleSwitch
        checked={audio}
        onChange={(value) => void configure(enabled, value)}
        isUpdating={busy}
        label={t("settings.learning.audio")}
        description={t("settings.learning.audioDescription")}
        descriptionMode="inline"
        grouped
      />
      <div className="p-4 space-y-3">
        <label htmlFor="learning-folder" className="text-sm font-semibold">
          {t("settings.learning.folder")}
        </label>
        <div className="flex items-center gap-2">
          <Input
            id="learning-folder"
            className="flex-1"
            value={directory}
            onChange={(event) => setDirectory(event.target.value)}
            placeholder="/absolute/path/to/learning"
          />
          <Button
            variant="secondary"
            size="sm"
            disabled={busy || !directory.trim()}
            onClick={() => void configure(enabled, audio)}
          >
            {t("common.save")}
          </Button>
        </div>
        <div className="flex gap-2">
          <Button
            variant="secondary"
            size="sm"
            disabled={!savedDirectory || busy}
            onClick={() => {
              void commands
                .openLearningFolder()
                .then((result) => {
                  if (result.status === "error") setError(result.error);
                })
                .catch((error) => setError(String(error)));
            }}
          >
            {t("settings.learning.openFolder")}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            disabled={busy}
            onClick={() => void refresh()}
          >
            {t("settings.learning.refresh")}
          </Button>
        </div>
        <p className="text-xs text-text/60">
          {t("settings.learning.noTraining")}
        </p>
        {error && (
          <p role="alert" className="text-sm text-red-500">
            {error}
          </p>
        )}
        {samples.length === 0 ? (
          <p className="text-sm text-text/60">{t("settings.learning.empty")}</p>
        ) : (
          <>
            <Dropdown
              selectedValue={selectedId}
              onSelect={setSelectedId}
              options={samples.map((sample) => ({
                value: sample.id,
                label: `${new Date(sample.created_at).toLocaleString()} — ${sample.output_text.slice(0, 60)}`,
              }))}
            />
            {selected && (
              <>
                <details className="text-sm space-y-2">
                  <summary className="cursor-pointer">
                    {t("settings.learning.compare")}
                  </summary>
                  <p className="font-semibold">{t("settings.learning.raw")}</p>
                  <p className="whitespace-pre-wrap text-text/70">
                    {selected.raw_transcript}
                  </p>
                  <p className="font-semibold">
                    {t("settings.learning.model")}
                  </p>
                  <p className="whitespace-pre-wrap text-text/70">
                    {selected.model_output ??
                      t("settings.learning.noPostProcess")}
                  </p>
                  <p className="text-xs text-text/60">
                    {selected.audio_saved
                      ? t("settings.learning.audioSaved")
                      : t("settings.learning.noAudio")}
                  </p>
                </details>
                <label
                  htmlFor="learning-correction"
                  className="block text-sm font-semibold"
                >
                  {t("settings.learning.correction")}
                </label>
                <Textarea
                  id="learning-correction"
                  value={correction}
                  onChange={(event) => {
                    setCorrection(event.target.value);
                    setSaved(false);
                  }}
                  className="w-full min-h-24"
                  placeholder={t("settings.learning.correctionPlaceholder")}
                />
                <Button
                  variant="primary"
                  size="sm"
                  disabled={busy}
                  onClick={() => void saveCorrection()}
                >
                  {t("settings.learning.saveCorrection")}
                </Button>
                {saved && (
                  <p role="status" className="text-sm text-green-600">
                    {t("settings.learning.saved")}
                  </p>
                )}
              </>
            )}
          </>
        )}
      </div>
    </SettingsGroup>
  );
};
