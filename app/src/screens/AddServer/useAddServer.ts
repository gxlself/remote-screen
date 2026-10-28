import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { RootScreenProps } from '../../app/navigation/routes';
import { showToast } from '../../components/ToastHost';
import { useCoreEvent } from '../../features/events/EventsProvider';
import { useSshKeys, useSshServers } from '../../features/data/queries';
import { newServerId } from '../../features/servers/manual';
import type { ManualServer } from '../../features/servers/manual';
import { useManualServers } from '../../features/servers/useManualServers';
import { useSetting } from '../../features/settings/store';
import {
  EMPTY_FORM,
  FIELD_ORDER,
  formToServer,
  hostKind,
  parseHostPort,
  requiredNetwork,
  serverToForm,
  testFromDiscover,
  testFromProbe,
  testTarget,
  validateServerForm,
} from '../../features/ssh/serverForm';
import type {
  FieldKey,
  FormErrorCode,
  ServerForm,
  TestState,
} from '../../features/ssh/serverForm';
import { useCore } from '../../native/rscore/CoreContext';
import { RsCoreError } from '../../native/rscore/RsCoreApi';
import type {
  CoreEventOf,
  SshAuth,
  SshServer,
} from '../../native/rscore/types';
import { addServer } from '../../i18n/zh/addServer';

const TEST_TIMEOUT_MS = 5000;
/** Same budget as the native RSP probe (`timeouts.probe`), which the timeout copy names. */
const TCP_PROBE_TIMEOUT_MS = 3000;

type TextFieldKey = 'name' | 'user' | 'host' | 'port' | 'jump';

export type JumpServer = {
  id: string;
  name: string;
  host: string;
  port: number;
};

type RunningProbe = {
  probeId: string;
  via: string | null;
  timer: ReturnType<typeof setTimeout>;
};

export function useAddServer({
  navigation,
  route,
}: RootScreenProps<'AddServer'>) {
  const core = useCore();
  const manual = useManualServers();
  const sshQuery = useSshServers();
  const keysQuery = useSshKeys();
  const [networks] = useSetting('networks');
  const serverId = route.params?.serverId;
  // Editing ignores the prefill; user and auth keep the new-server defaults.
  const prefill = serverId === undefined ? route.params?.prefill : undefined;
  const connectAfterSave = prefill?.connectAfterSave === true;

  const [initialForm] = useState<ServerForm>(() =>
    prefill === undefined
      ? EMPTY_FORM
      : {
          ...EMPTY_FORM,
          name: prefill.name,
          host: prefill.host,
          port:
            prefill.port === undefined ? EMPTY_FORM.port : String(prefill.port),
        },
  );
  const [form, setForm] = useState<ServerForm>(initialForm);
  /** What the form held when it opened; leaving with changes asks first. */
  const [baseline, setBaseline] = useState<{
    form: ServerForm;
    networkChoice: string | null | undefined;
  }>({ form: initialForm, networkChoice: undefined });
  const [initialized, setInitialized] = useState(serverId === undefined);
  const [notFound, setNotFound] = useState(false);
  const [readOnly, setReadOnly] = useState(false);
  const [editingId, setEditingId] = useState<string | undefined>(serverId);
  const [editingName, setEditingName] = useState<string | undefined>(undefined);
  const [editingAddedAtUs, setEditingAddedAtUs] = useState<number | undefined>(
    undefined,
  );
  const [networkChoice, setNetworkChoice] = useState<string | null | undefined>(
    undefined,
  );
  const [submitted, setSubmitted] = useState(false);
  const [testState, setTestState] = useState<TestState>({ kind: 'idle' });
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  /** Set once a save or delete went through, so leaving does not ask. */
  const leaving = useRef(false);

  const probeRef = useRef<RunningProbe | null>(null);
  /** Final results that arrived before `probeAddress` returned their id. */
  const earlyEvents = useRef<Map<string, CoreEventOf<'probe.result'>> | null>(
    null,
  );
  const requestRef = useRef(0);

  const sshServers = useMemo<SshServer[]>(
    () => sshQuery.data ?? [],
    [sshQuery.data],
  );

  useEffect(() => {
    if (serverId === undefined || initialized) {
      return;
    }
    if (manual.isLoading || sshQuery.isLoading) {
      return;
    }

    const local = manual.servers.find(server => server.id === serverId);
    if (local !== undefined) {
      const loaded = serverToForm(local);
      setForm(loaded);
      setBaseline({ form: loaded, networkChoice: local.networkId });
      setEditingId(local.id);
      setEditingName(local.name);
      setEditingAddedAtUs(local.addedAtUs);
      setNetworkChoice(local.networkId);
      setReadOnly(false);
      setInitialized(true);
      return;
    }

    const discovered = sshServers.find(server => server.serverId === serverId);
    if (discovered === undefined) {
      setNotFound(true);
      setInitialized(true);
      return;
    }

    const loaded = serverToForm(discovered);
    setForm(loaded);
    setBaseline({ form: loaded, networkChoice: undefined });
    setEditingId(discovered.serverId);
    setEditingName(discovered.name);
    setReadOnly(discovered.source === 'directory');
    setInitialized(true);
  }, [
    initialized,
    manual.isLoading,
    manual.servers,
    serverId,
    sshQuery.isLoading,
    sshServers,
  ]);

  const jumpServers = useMemo<JumpServer[]>(() => {
    const merged = new Map<string, JumpServer>();
    for (const server of sshServers) {
      merged.set(server.serverId, {
        id: server.serverId,
        name: server.name,
        host: server.host,
        port: server.port,
      });
    }
    for (const server of manual.servers) {
      if (!merged.has(server.id)) {
        merged.set(server.id, {
          id: server.id,
          name: server.name,
          host: server.host,
          port: server.port,
        });
      }
    }
    return [...merged.values()].filter(server => server.id !== editingId);
  }, [editingId, manual.servers, sshServers]);

  const existingNames = useMemo(() => {
    const names = new Set<string>();
    for (const server of manual.servers) {
      names.add(server.name);
    }
    for (const server of sshServers) {
      names.add(server.name);
    }
    return [...names];
  }, [manual.servers, sshServers]);

  const existingIds = useMemo(() => {
    const ids = new Set<string>();
    for (const server of manual.servers) {
      ids.add(server.id);
    }
    for (const server of sshServers) {
      ids.add(server.serverId);
    }
    return [...ids];
  }, [manual.servers, sshServers]);

  const keys = keysQuery.data;
  const hasKey = keys === undefined ? undefined : keys.length > 0;
  const jumpNames = useMemo(
    () => jumpServers.map(server => server.name),
    [jumpServers],
  );
  const validation = useMemo(
    () =>
      validateServerForm(form, {
        existingNames,
        editingName,
        hasKey,
        jumpNames,
      }),
    [editingName, existingNames, form, hasKey, jumpNames],
  );
  const errors: Partial<Record<FieldKey, FormErrorCode>> = submitted
    ? validation
    : {};
  const firstError = FIELD_ORDER.find(key => errors[key] !== undefined) ?? null;

  const autoNetworkId = requiredNetwork(form.host, networks)?.id ?? null;
  const networkId = networkChoice === undefined ? autoNetworkId : networkChoice;
  const dirty =
    (Object.keys(form) as Array<keyof ServerForm>).some(
      key => form[key] !== baseline.form[key],
    ) || networkChoice !== baseline.networkChoice;

  const cancelProbe = useCallback(() => {
    requestRef.current += 1;
    earlyEvents.current = null;
    const active = probeRef.current;
    if (active !== null) {
      clearTimeout(active.timer);
      probeRef.current = null;
    }
    setTestState(current =>
      current.kind === 'idle' ? current : { kind: 'idle' },
    );
  }, []);

  useEffect(
    () => () => {
      requestRef.current += 1;
      earlyEvents.current = null;
      const active = probeRef.current;
      if (active !== null) {
        clearTimeout(active.timer);
        probeRef.current = null;
      }
    },
    [],
  );

  useCoreEvent('probe.result', event => {
    if (!event.last) {
      return;
    }
    const active = probeRef.current;
    if (active === null || active.probeId !== event.probeId) {
      earlyEvents.current?.set(event.probeId, event);
      return;
    }
    clearTimeout(active.timer);
    probeRef.current = null;
    setTestState(testFromProbe(event, active.via));
  });

  const setField = useCallback(
    (key: TextFieldKey, value: string) => {
      cancelProbe();
      setSaveError(null);
      setForm(current => ({ ...current, [key]: value }));
    },
    [cancelProbe],
  );

  const setAuth = useCallback(
    (auth: SshAuth) => {
      cancelProbe();
      setSaveError(null);
      setForm(current => ({ ...current, auth }));
    },
    [cancelProbe],
  );

  const setJumpOn = useCallback(
    (jumpOn: boolean) => {
      cancelProbe();
      setSaveError(null);
      setForm(current => ({ ...current, jumpOn }));
    },
    [cancelProbe],
  );

  const selectNetwork = useCallback((id: string | null) => {
    setNetworkChoice(id);
  }, []);

  const currentJump = form.jump.trim();
  const resolvedJump = jumpServers.find(server => server.name === currentJump);
  const jumpAddress =
    resolvedJump === undefined
      ? null
      : `${resolvedJump.host}:${resolvedJump.port}`;

  const testConnection = useCallback(async () => {
    setSubmitted(true);
    if (Object.keys(validation).length > 0) {
      return;
    }
    const target = testTarget(form);
    if (target === null) {
      return;
    }

    let probeHost = target.host;
    let probePort = target.port;
    if (form.jumpOn) {
      const jump = parseHostPort(form.jump, 22);
      if (jump !== null) {
        probeHost = jump.host;
        probePort = jump.port;
      }
    }

    const request = requestRef.current + 1;
    requestRef.current = request;
    setSaveError(null);
    setTestState({ kind: 'running', probeId: null });

    try {
      const discover = core.discover;
      if (discover !== undefined) {
        const result = await discover.probe(
          probeHost,
          probePort,
          'tcp',
          TCP_PROBE_TIMEOUT_MS,
        );
        if (requestRef.current === request) {
          setTestState(testFromDiscover(result, target.via));
        }
        return;
      }

      earlyEvents.current = new Map();
      const probeId = await core.diag.probeAddress(probeHost, probePort);
      if (requestRef.current !== request) {
        return;
      }
      const early = earlyEvents.current?.get(probeId);
      earlyEvents.current = null;
      if (early !== undefined) {
        setTestState(testFromProbe(early, target.via));
        return;
      }
      const timer = setTimeout(() => {
        if (probeRef.current?.probeId !== probeId) {
          return;
        }
        probeRef.current = null;
        setTestState({
          kind: 'fail',
          reason: 'noResult',
          via: target.via,
        });
      }, TEST_TIMEOUT_MS);
      probeRef.current = { probeId, via: target.via, timer };
      setTestState({ kind: 'running', probeId });
    } catch (error) {
      if (requestRef.current !== request) {
        return;
      }
      earlyEvents.current = null;
      const reason =
        error instanceof RsCoreError && error.kind === 'unsupported'
          ? 'unsupported'
          : 'skipped';
      setTestState({ kind: 'fail', reason, via: target.via });
    }
  }, [core.diag, core.discover, form, validation]);

  /** Resolves to the first field that still needs fixing, if any. */
  const save = useCallback(async (): Promise<FieldKey | null> => {
    setSubmitted(true);
    setSaveError(null);
    const invalid = FIELD_ORDER.find(key => validation[key] !== undefined);
    if (invalid !== undefined) {
      return invalid;
    }

    const id = editingId ?? newServerId(form.name, existingIds);
    const nowUs = Date.now() * 1000;
    const built = formToServer(form, {
      id,
      nowUs,
      addedAtUs: editingAddedAtUs,
      networks,
    });
    const server: ManualServer = { ...built, networkId };

    setSaving(true);
    try {
      await manual.saveServer(server);
    } catch {
      setSaveError(addServer.save.failed);
      setSaving(false);
      return null;
    }
    setSaving(false);
    leaving.current = true;
    if (connectAfterSave) {
      // Same route as tapping the server on Home.
      navigation.replace('Terminal', { serverId: server.id });
    } else {
      showToast({
        message:
          editingId === undefined ? addServer.save.added : addServer.save.saved,
        tone: 'good',
      });
      navigation.goBack();
    }
    return null;
  }, [
    connectAfterSave,
    editingAddedAtUs,
    editingId,
    existingIds,
    form,
    manual,
    navigation,
    networkId,
    networks,
    validation,
  ]);

  const remove = useCallback(async (): Promise<boolean> => {
    if (editingId === undefined || deleting) {
      return false;
    }
    setDeleting(true);
    setDeleteError(null);
    try {
      await manual.removeServer(editingId);
    } catch {
      setDeleteError(addServer.delete.failed);
      setDeleting(false);
      return false;
    }
    setDeleting(false);
    leaving.current = true;
    navigation.goBack();
    return true;
  }, [deleting, editingId, manual, navigation]);

  const tailscaleUnavailable =
    form.auth === 'tailscale' &&
    (form.jumpOn || !['tailnet', 'magicDns'].includes(hostKind(form.host)));

  return {
    serverId,
    prefilled: prefill !== undefined,
    connectAfterSave,
    editingId,
    editingName,
    form,
    errors,
    networks,
    networkId,
    jumpServers,
    jumpAddress,
    tailscaleUnavailable,
    testState,
    savedError: saveError,
    deleteError,
    saving,
    deleting,
    firstError,
    dirty,
    leaving,
    notFound,
    readOnly,
    loading: serverId !== undefined && !initialized,
    setField,
    setAuth,
    setJumpOn,
    selectNetwork,
    testConnection,
    save,
    remove,
  };
}

export type AddServerController = ReturnType<typeof useAddServer>;
