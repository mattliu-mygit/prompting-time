export type ProviderId = "codex" | "claude";

export interface ProviderInstallation {
  id: ProviderId;
  installed: boolean;
  version: string | null;
  diagnostic: string | null;
}

export interface BootstrapSnapshot {
  providers: ProviderInstallation[];
}
