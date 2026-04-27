import { useEffect, useState } from "react";
import {
  AlertTriangle,
  Check,
  ChevronDown,
  Clock,
  KeyRound,
  Loader2,
  Server,
  ShieldAlert,
  ShieldCheck,
  X,
} from "lucide-react";
import { tauri } from "../lib/tauri";
import { useVizStore } from "../store/vizStore";
import type {
  HostKeyChangedInfo,
  RecentRemote,
  SshAgentProbe,
  SshHostEntry,
  TestResult,
  TestStage,
  UnknownHostInfo,
} from "../types";

const DEFAULT_GLOB = "**/*.{png,jpg,jpeg,webp,gif,svg,html,pdf,csv}";

interface Props {
  onClose: () => void;
}

export function ConnectRemoteDialog({ onClose }: Props) {
  const addWatch = useVizStore((s) => s.addWatch);

  const [agent, setAgent] = useState<SshAgentProbe | null>(null);
  const [hosts, setHosts] = useState<SshHostEntry[]>([]);
  const [recents, setRecents] = useState<RecentRemote[]>([]);
  const [selectedAlias, setSelectedAlias] = useState<string>("");
  const [host, setHost] = useState("");
  const [user, setUser] = useState("");
  const [port, setPort] = useState<string>("22");
  const [remotePath, setRemotePath] = useState("");
  const [glob, setGlob] = useState(DEFAULT_GLOB);

  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<TestResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [topErr, setTopErr] = useState<string | null>(null);
  const [trusting, setTrusting] = useState(false);

  useEffect(() => {
    tauri.probeSshAgent().then(setAgent).catch(() =>
      setAgent({ available: false, key_count: 0, error: "probe failed" }),
    );
    tauri.listSshHosts().then(setHosts).catch(() => setHosts([]));
    tauri.listRecentRemotes().then(setRecents).catch(() => setRecents([]));
  }, []);

  const onPickRecent = (r: RecentRemote) => {
    setSelectedAlias("");
    setHost(r.host);
    setUser(r.user);
    setPort(String(r.port));
    setRemotePath(r.remote_path);
    setGlob(r.glob);
    setTestResult(null);
    setTopErr(null);
  };

  const onForgetRecent = async (r: RecentRemote, e: React.MouseEvent) => {
    e.stopPropagation();
    await tauri.forgetRecentRemote(r.host, r.user, r.port, r.remote_path).catch(() => {});
    setRecents((prev) =>
      prev.filter(
        (x) =>
          !(
            x.host === r.host &&
            x.user === r.user &&
            x.port === r.port &&
            x.remote_path === r.remote_path
          ),
      ),
    );
  };

  const onPickAlias = (alias: string) => {
    setSelectedAlias(alias);
    if (!alias) return;
    const entry = hosts.find((h) => h.alias === alias);
    if (entry) {
      setHost(entry.host_name);
      setUser(entry.user ?? "");
      setPort(String(entry.port));
    }
  };

  const onTest = async () => {
    setTopErr(null);
    setTestResult(null);
    setTesting(true);
    try {
      const portNum = port.trim() ? parseInt(port, 10) : null;
      const r = await tauri.testSshConnection({
        host: selectedAlias || host,
        user: user.trim() || null,
        port: portNum && !isNaN(portNum) ? portNum : null,
        remote_path: remotePath.trim() || null,
        glob,
      });
      setTestResult(r);
    } catch (e) {
      setTopErr(String(e));
    } finally {
      setTesting(false);
    }
  };

  const onTrustUnknown = async (info: UnknownHostInfo) => {
    setTopErr(null);
    setTrusting(true);
    try {
      await tauri.confirmUnknownHost(info.host, info.port, info.fingerprint);
      // Re-run the test so the user immediately sees auth/path/match stages succeed.
      await onTest();
    } catch (e) {
      setTopErr(String(e));
    } finally {
      setTrusting(false);
    }
  };

  const onConnect = async () => {
    setTopErr(null);
    setBusy(true);
    try {
      const portNum = port.trim() ? parseInt(port, 10) : null;
      const watch = await tauri.addRemoteWatch({
        host: selectedAlias || host,
        user: user.trim() || null,
        port: portNum && !isNaN(portNum) ? portNum : null,
        remote_path: remotePath.trim() || null,
        glob,
      });
      addWatch(watch);
      onClose();
    } catch (e) {
      setTopErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const hasUnresolvedHostKeyIssue = !!(
    testResult?.unknown_host || testResult?.host_key_changed
  );
  const canTest = !!(host || selectedAlias) && !!glob && !testing && !trusting;
  const canConnect =
    !!(host || selectedAlias) &&
    !!glob &&
    !busy &&
    !trusting &&
    !hasUnresolvedHostKeyIssue &&
    (agent?.available ?? false);

  return (
    <div
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4"
      onClick={onClose}
    >
      <div
        className="w-full max-w-xl rounded-lg border border-[color:var(--color-border)] bg-[color:var(--color-bg)] shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-3 border-b border-[color:var(--color-border)]">
          <div className="flex items-center gap-2">
            <Server className="w-4 h-4 text-[color:var(--color-accent)]" />
            <span className="text-[14px] font-semibold">Connect to a remote server</span>
          </div>
          <button
            onClick={onClose}
            className="text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)]"
            aria-label="Close"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        <div className="px-5 py-4 space-y-4">
          <AgentBanner probe={agent} />

          {recents.length > 0 && (
            <RecentList
              recents={recents}
              onPick={onPickRecent}
              onForget={onForgetRecent}
            />
          )}

          {hosts.length > 0 && (
            <Field label="From ~/.ssh/config" hint="Picks defaults; override below if needed.">
              <div className="relative">
                <select
                  value={selectedAlias}
                  onChange={(e) => onPickAlias(e.target.value)}
                  className="w-full appearance-none px-3 py-2 pr-8 rounded border border-[color:var(--color-border)] bg-[color:var(--color-surface)] text-[13px] focus:outline-none focus:border-[color:var(--color-accent)]/60"
                >
                  <option value="">— select a host —</option>
                  {hosts.map((h) => (
                    <option key={h.alias} value={h.alias}>
                      {h.alias}
                      {h.user ? ` (${h.user}@${h.host_name})` : ` (${h.host_name})`}
                    </option>
                  ))}
                </select>
                <ChevronDown className="w-3.5 h-3.5 absolute right-2.5 top-1/2 -translate-y-1/2 text-[color:var(--color-text-dim)] pointer-events-none" />
              </div>
            </Field>
          )}

          <div className="grid grid-cols-[1fr_120px_80px] gap-2">
            <Field label="Host">
              <Input
                value={host}
                onChange={setHost}
                placeholder="hostname or IP"
              />
            </Field>
            <Field label="User">
              <Input value={user} onChange={setUser} placeholder="optional" />
            </Field>
            <Field label="Port">
              <Input value={port} onChange={setPort} placeholder="22" />
            </Field>
          </div>

          <Field
            label="Remote path"
            hint="Optional. Defaults to the home directory — change folder after connecting."
          >
            <Input
              value={remotePath}
              onChange={setRemotePath}
              placeholder="(home directory)"
            />
          </Field>

          <Field label="Glob" hint="Files matching this pattern under the remote path will appear.">
            <Input value={glob} onChange={setGlob} placeholder={DEFAULT_GLOB} />
          </Field>

          <div className="flex items-center gap-2 pt-1">
            <button
              type="button"
              onClick={onTest}
              disabled={!canTest}
              className="px-3 py-1.5 rounded border border-[color:var(--color-border)] bg-[color:var(--color-surface)] hover:border-[color:var(--color-accent)]/60 text-[12px] disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-1.5"
            >
              {testing ? (
                <Loader2 className="w-3.5 h-3.5 animate-spin" />
              ) : (
                <Check className="w-3.5 h-3.5" />
              )}
              Test connection
            </button>
            {testResult && <TestResultView result={testResult} />}
          </div>

          {testResult?.unknown_host && (
            <UnknownHostPanel
              info={testResult.unknown_host}
              trusting={trusting}
              onTrust={onTrustUnknown}
              onCancel={() => setTestResult(null)}
            />
          )}

          {testResult?.host_key_changed && (
            <HostKeyChangedPanel info={testResult.host_key_changed} />
          )}

          {topErr && (
            <div className="text-[12px] text-red-300 flex items-start gap-1.5">
              <AlertTriangle className="w-3.5 h-3.5 mt-0.5 flex-shrink-0" />
              <span className="whitespace-pre-wrap break-words">{topErr}</span>
            </div>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 px-5 py-3 border-t border-[color:var(--color-border)]">
          <button
            type="button"
            onClick={onClose}
            disabled={busy}
            className="px-3 py-1.5 rounded text-[12px] text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)] disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConnect}
            disabled={!canConnect}
            className="px-3 py-1.5 rounded text-[12px] font-medium bg-[color:var(--color-accent)] text-black hover:opacity-90 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-1.5"
          >
            {busy && <Loader2 className="w-3.5 h-3.5 animate-spin" />}
            Connect
          </button>
        </div>
      </div>
    </div>
  );
}

function AgentBanner({ probe }: { probe: SshAgentProbe | null }) {
  if (!probe) {
    return (
      <div className="flex items-center gap-2 text-[12px] text-[color:var(--color-text-dim)]">
        <Loader2 className="w-3.5 h-3.5 animate-spin" />
        Probing ssh-agent…
      </div>
    );
  }
  if (probe.available) {
    return (
      <div className="flex items-center gap-2 text-[12px] text-[color:var(--color-text-dim)]">
        <KeyRound className="w-3.5 h-3.5 text-[color:var(--color-accent)]" />
        ssh-agent: {probe.key_count} {probe.key_count === 1 ? "key" : "keys"} loaded
      </div>
    );
  }
  return (
    <div className="text-[12px] text-amber-300/90 flex items-start gap-1.5 leading-snug">
      <AlertTriangle className="w-3.5 h-3.5 mt-0.5 flex-shrink-0" />
      <div>
        No ssh-agent detected.{" "}
        {probe.error && <span className="opacity-70">({probe.error}) </span>}
        Run <code className="font-mono px-1 rounded bg-[color:var(--color-surface-2)]">ssh-add ~/.ssh/id_ed25519</code> in
        a terminal, then reopen this dialog.
      </div>
    </div>
  );
}

function TestResultView({ result }: { result: TestResult }) {
  return (
    <div className="flex items-center gap-2 text-[11px] text-[color:var(--color-text-dim)]">
      <StageBadge label="reachable" stage={result.reachable} />
      <StageBadge label="auth" stage={result.authenticated} />
      <StageBadge label="path" stage={result.path_exists} />
      <StageBadge
        label={
          result.matched.matched_files != null
            ? `match (${result.matched.matched_files})`
            : "match"
        }
        stage={result.matched}
      />
    </div>
  );
}

function StageBadge({ label, stage }: { label: string; stage: TestStage }) {
  const ok = stage.ok;
  const color = ok ? "text-emerald-300" : "text-red-300/90";
  const icon = ok ? <Check className="w-3 h-3" /> : <X className="w-3 h-3" />;
  return (
    <span className={`flex items-center gap-1 ${color}`} title={stage.error ?? "ok"}>
      {icon}
      {label}
    </span>
  );
}

function RecentList({
  recents,
  onPick,
  onForget,
}: {
  recents: RecentRemote[];
  onPick: (r: RecentRemote) => void;
  onForget: (r: RecentRemote, e: React.MouseEvent) => void;
}) {
  return (
    <div>
      <div className="flex items-center gap-1.5 mb-1.5">
        <Clock className="w-3 h-3 text-[color:var(--color-text-dim)]" />
        <span className="text-[11px] uppercase tracking-wider text-[color:var(--color-text-dim)]">
          Recent
        </span>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {recents.map((r) => {
          const k = `${r.host}|${r.user}|${r.port}|${r.remote_path}`;
          return (
            <button
              key={k}
              type="button"
              onClick={() => onPick(r)}
              title={`${r.user}@${r.host}:${r.port} ${r.remote_path}`}
              className="group flex items-center gap-1.5 max-w-[280px] px-2 py-1 rounded border border-[color:var(--color-border)] bg-[color:var(--color-surface)] hover:border-[color:var(--color-accent)]/60 hover:bg-[color:var(--color-surface-2)] text-[11px] text-left"
            >
              <span className="font-mono truncate">
                <span className="opacity-70">{r.user}@</span>
                {r.host}
                <span className="opacity-50">:{r.remote_path}</span>
              </span>
              <span
                role="button"
                tabIndex={0}
                onClick={(e) => onForget(r, e)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") onForget(r, e as unknown as React.MouseEvent);
                }}
                title="Forget this connection"
                aria-label="Forget this connection"
                className="opacity-40 hover:opacity-100 hover:text-red-300 cursor-pointer"
              >
                <X className="w-3 h-3" />
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <div className="flex items-baseline justify-between">
        <label className="text-[11px] uppercase tracking-wider text-[color:var(--color-text-dim)]">
          {label}
        </label>
        {hint && <span className="text-[10px] text-[color:var(--color-text-dim)]">{hint}</span>}
      </div>
      <div className="mt-1">{children}</div>
    </div>
  );
}

function Input({
  value,
  onChange,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  return (
    <input
      type="text"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      className="w-full px-3 py-2 rounded border border-[color:var(--color-border)] bg-[color:var(--color-surface)] text-[13px] font-mono placeholder:opacity-40 focus:outline-none focus:border-[color:var(--color-accent)]/60"
    />
  );
}

function UnknownHostPanel({
  info,
  trusting,
  onTrust,
  onCancel,
}: {
  info: UnknownHostInfo;
  trusting: boolean;
  onTrust: (info: UnknownHostInfo) => void;
  onCancel: () => void;
}) {
  const [showFull, setShowFull] = useState(false);
  return (
    <div className="rounded border border-amber-500/60 bg-amber-500/5 p-3 space-y-2">
      <div className="flex items-start gap-2">
        <ShieldAlert className="w-4 h-4 mt-0.5 flex-shrink-0 text-amber-300" />
        <div className="text-[12px] leading-snug">
          <div className="font-semibold text-amber-200">Unknown host</div>
          <div className="text-[color:var(--color-text-dim)] mt-0.5">
            This is the first time you're connecting to{" "}
            <code className="font-mono px-1 rounded bg-[color:var(--color-surface-2)]">
              {info.host}:{info.port}
            </code>
            . Verify the fingerprint below matches what your devbox admin (or{" "}
            <code className="font-mono">ssh-keyscan</code>) reports before trusting.
          </div>
        </div>
      </div>
      <div className="grid grid-cols-[80px_1fr] gap-x-3 gap-y-1 text-[11px] font-mono pl-6">
        <span className="text-[color:var(--color-text-dim)]">key type</span>
        <span>{info.key_type}</span>
        <span className="text-[color:var(--color-text-dim)]">fingerprint</span>
        <span className="break-all">{info.fingerprint}</span>
      </div>
      <div className="pl-6">
        <button
          type="button"
          onClick={() => setShowFull((v) => !v)}
          className="text-[11px] text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)] underline-offset-2 hover:underline"
        >
          {showFull ? "Hide full key" : "Show full key"}
        </button>
        {showFull && (
          <pre className="mt-1 p-2 text-[10px] font-mono whitespace-pre-wrap break-all bg-[color:var(--color-surface-2)] rounded border border-[color:var(--color-border)]">
            {info.raw_openssh}
          </pre>
        )}
      </div>
      <div className="flex items-center gap-2 pl-6 pt-1">
        <button
          type="button"
          onClick={() => onTrust(info)}
          disabled={trusting}
          className="px-2.5 py-1 rounded text-[11px] font-medium bg-amber-500/80 text-black hover:bg-amber-400 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-1.5"
        >
          {trusting ? (
            <Loader2 className="w-3 h-3 animate-spin" />
          ) : (
            <ShieldCheck className="w-3 h-3" />
          )}
          Trust and add to known_hosts
        </button>
        <button
          type="button"
          onClick={onCancel}
          disabled={trusting}
          className="px-2.5 py-1 rounded text-[11px] text-[color:var(--color-text-dim)] hover:text-[color:var(--color-text)] disabled:opacity-50"
        >
          Cancel
        </button>
      </div>
    </div>
  );
}

function HostKeyChangedPanel({ info }: { info: HostKeyChangedInfo }) {
  const cmd = `ssh-keygen -R '${info.host}'`;
  return (
    <div className="rounded border border-red-500/70 bg-red-500/10 p-3 space-y-2">
      <div className="flex items-start gap-2">
        <AlertTriangle className="w-4 h-4 mt-0.5 flex-shrink-0 text-red-300" />
        <div className="text-[12px] leading-snug">
          <div className="font-semibold text-red-200">Host key changed</div>
          <div className="text-[color:var(--color-text-dim)] mt-0.5">
            The key{" "}
            <code className="font-mono px-1 rounded bg-[color:var(--color-surface-2)]">
              {info.host}:{info.port}
            </code>{" "}
            is presenting does not match the one in{" "}
            <code className="font-mono">~/.ssh/known_hosts</code> (line {info.known_hosts_line}).
            This could be a re-imaged box — or someone in the middle. Confirm with the host owner
            before doing anything else.
          </div>
        </div>
      </div>
      <div className="grid grid-cols-[80px_1fr] gap-x-3 gap-y-1 text-[11px] font-mono pl-6">
        <span className="text-[color:var(--color-text-dim)]">offered</span>
        <span className="break-all">{info.fingerprint_offered}</span>
        <span className="text-[color:var(--color-text-dim)]">known</span>
        <span className="break-all">{info.fingerprint_known}</span>
      </div>
      <div className="pl-6 space-y-1">
        <div className="text-[11px] text-[color:var(--color-text-dim)]">
          To re-trust after verifying out-of-band, remove the stale entry first:
        </div>
        <code className="block font-mono text-[11px] px-2 py-1 bg-[color:var(--color-surface-2)] rounded border border-[color:var(--color-border)] select-all">
          {cmd}
        </code>
      </div>
    </div>
  );
}
