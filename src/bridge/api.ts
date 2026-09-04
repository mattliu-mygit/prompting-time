import { invoke } from "@tauri-apps/api/core";
import type { BootstrapSnapshot } from "./types";

export function getBootstrap(): Promise<BootstrapSnapshot> {
  return invoke<BootstrapSnapshot>("bootstrap");
}
