import { invoke } from "@tauri-apps/api/core";

/// iPhone notifications through Pushover (src-tauri/src/commands/push.rs).

export interface PushConfig {
  enabled: boolean;
  replies: boolean;
  out_of_office: boolean;
  epk: boolean;
  opens: boolean;
  /// Opens counted in the daily summary instead of one push each.
  opens_in_summary: boolean;
  problems: boolean;
  summary: boolean;
  quiet_enabled: boolean;
  /// "HH:MM", Paris time.
  quiet_start: string;
  quiet_end: string;
}

export interface PushView {
  config: PushConfig;
  /// Masked ("••••••••") when set, "" otherwise.
  user: string;
  token: string;
  last_error: string | null;
  last_error_at: string | null;
  last_sent_at: string | null;
  /// Pushover refused the keys: nothing is sent until they are saved or tested.
  blocked: boolean;
  /// Notices waiting for the end of quiet hours.
  held: number;
  sent_today: number;
}

export const pushGet = () => invoke<PushView>("push_get");
export const pushSave = (config: PushConfig, user: string, token: string) =>
  invoke<PushView>("push_save", { config, user, token });
export const pushTest = () => invoke<PushView>("push_test");
