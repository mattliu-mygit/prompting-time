import { useEffect, useState } from "react";
import type { BootstrapSnapshot, ProviderInstallation } from "../bridge/types";

type AppProps = {
  bootstrap: () => Promise<BootstrapSnapshot>;
};

const providerNames = {
  codex: "Codex",
  claude: "Claude"
} as const;

export function App({ bootstrap }: AppProps) {
  const [snapshot, setSnapshot] = useState<BootstrapSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let active = true;

    bootstrap().then(
      (nextSnapshot) => {
        if (active) {
          setSnapshot(nextSnapshot);
          setError(null);
        }
      },
      (reason: unknown) => {
        if (active) {
          setError(reason instanceof Error ? reason.message : "Unknown error");
        }
      }
    );

    return () => {
      active = false;
    };
  }, [attempt, bootstrap]);

  if (error) {
    return (
      <main>
        <h1>Provider diagnostics</h1>
        <p>Could not inspect installed providers: {error}</p>
        <button onClick={() => setAttempt((currentAttempt) => currentAttempt + 1)}>Retry</button>
      </main>
    );
  }

  if (!snapshot) {
    return (
      <main>
        <h1>Provider diagnostics</h1>
        <p>Checking installed providers…</p>
      </main>
    );
  }

  return (
    <main>
      <h1>Provider diagnostics</h1>
      <ul>
        {snapshot.providers.map((provider) => (
          <ProviderDiagnostic key={provider.id} provider={provider} />
        ))}
      </ul>
    </main>
  );
}

function ProviderDiagnostic({ provider }: { provider: ProviderInstallation }) {
  const name = providerNames[provider.id];

  if (provider.installed && provider.version) {
    return <li>{`${name} ${provider.version}`}</li>;
  }

  return <li>{`${name} unavailable: ${provider.diagnostic ?? "Not installed"}`}</li>;
}
