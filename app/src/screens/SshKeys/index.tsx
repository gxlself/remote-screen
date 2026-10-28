import { useState } from 'react';
import { Pressable, ScrollView, StyleSheet, View } from 'react-native';
import { useQueryClient } from '@tanstack/react-query';
import {
  SafeAreaView,
  useSafeAreaInsets,
} from 'react-native-safe-area-context';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import Dialog from '../../components/Dialog';
import { EmptyState } from '../../components/EmptyState';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { StatusChip } from '../../components/StatusChip';
import Toast, { useToast } from '../../components/Toast';
import type { RootScreenProps } from '../../app/navigation/routes';
import {
  useSshKeys,
  useSshServers,
  useTrustedHostKeys,
} from '../../features/data/queries';
import { qk } from '../../features/data/queryClient';
import {
  defaultKeyName,
  errorMessage,
  installRows,
  isTailscaleSsh,
  isUnsupported,
  trustedRows,
} from '../../features/ssh/keysView';
import { common } from '../../i18n/zh/common';
import { sshKeys } from '../../i18n/zh/sshKeys';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';
import { useTailnetEnabled } from '../../features/tailnet/enabled';
import type { SshServer } from '../../native/rscore/types';
import { isAndroid, press } from '../../theme/platform';
import { hit, space, tokens } from '../../theme/tokens';
import InstallDialog from './InstallDialog';
import KeyCard from './KeyCard';

export default function SshKeysScreen({
  navigation,
}: RootScreenProps<'SshKeys'>) {
  const insets = useSafeAreaInsets();
  const core = useCore();
  const platform = usePlatform();
  const queryClient = useQueryClient();
  const { toast, show, hide } = useToast();

  const keysQuery = useSshKeys();
  const serversQuery = useSshServers();
  const trustedQuery = useTrustedHostKeys();

  const key = keysQuery.data?.[0] ?? null;
  const servers = serversQuery.data ?? [];
  const installView = installRows(servers, key);
  const trustRows = trustedRows(trustedQuery.data ?? [], servers);

  const tailnetEnabled = useTailnetEnabled();
  const [generating, setGenerating] = useState(false);
  const [installTarget, setInstallTarget] = useState<SshServer | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installedNow, setInstalledNow] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const [forgetTarget, setForgetTarget] = useState<{
    serverId: string;
    name: string;
  } | null>(null);
  const [forgetting, setForgetting] = useState(false);

  const generate = async () => {
    if (generating) {
      return;
    }
    setGenerating(true);
    try {
      const model = platform.deviceInfo().model;
      // The key is the device identity's; make sure it exists first.
      await core.identity.ensure(model);
      await core.ssh.createKey('se-p256', defaultKeyName(model));
      await queryClient.invalidateQueries({ queryKey: qk.sshKeys });
      show(sshKeys.empty.generated, { tone: 'good' });
    } catch (error) {
      show(sshKeys.empty.generateFailed(errorMessage(error)), { tone: 'bad' });
    } finally {
      setGenerating(false);
    }
  };

  const install = async () => {
    if (installTarget === null || key === null || installing) {
      return;
    }
    const target = installTarget;
    setInstalling(true);
    try {
      await core.ssh.installKey(target.serverId, key.keyId);
      await queryClient.invalidateQueries({ queryKey: qk.sshKeys });
      setInstalledNow(previous => new Set(previous).add(target.serverId));
      setInstallTarget(null);
      show(sshKeys.installDialog.success, { tone: 'good' });
    } catch (error) {
      setInstallTarget(null);
      show(sshKeys.installDialog.failed(errorMessage(error)), { tone: 'bad' });
    } finally {
      setInstalling(false);
    }
  };

  const forget = async () => {
    if (forgetTarget === null || forgetting) {
      return;
    }
    const target = forgetTarget;
    setForgetting(true);
    try {
      await core.ssh.forgetHostKey(target.serverId);
      await queryClient.invalidateQueries({ queryKey: qk.trustedHostKeys });
      setForgetTarget(null);
      show(sshKeys.trusted.forgotten, { tone: 'good' });
    } catch (error) {
      setForgetTarget(null);
      show(
        isUnsupported(error)
          ? sshKeys.trusted.forgetUnsupported
          : sshKeys.trusted.forgetFailed(errorMessage(error)),
        { tone: 'bad' },
      );
    } finally {
      setForgetting(false);
    }
  };

  const trustError = trustedQuery.error;
  const trustUnsupported = trustError != null && isUnsupported(trustError);
  const trustFailed =
    trustError != null && !trustUnsupported && !trustedQuery.isLoading;
  // Tailscale SSH rows and hints only exist while Tailscale is switched on.
  const tailscaleServers = tailnetEnabled ? servers.filter(isTailscaleSsh) : [];
  const trustedCount =
    tailscaleServers.length + (trustUnsupported ? 0 : trustRows.length);
  const trustEmpty =
    trustedCount === 0 && !trustFailed && !trustedQuery.isLoading;
  const firstTailscale = tailnetEnabled
    ? installView.tailscaleNames[0]
    : undefined;

  const installAction = (serverId: string) => {
    const server = servers.find(candidate => candidate.serverId === serverId);
    if (server !== undefined) {
      setInstallTarget(server);
    }
  };

  return (
    <SafeAreaView edges={['top']} style={styles.screen}>
      <ScrollView
        contentContainerStyle={[
          styles.content,
          { paddingBottom: space.gutter * 2 + insets.bottom },
        ]}
        showsVerticalScrollIndicator={false}
        testID="ssh-keys-scroll"
      >
        <View style={styles.header}>
          <Pressable
            accessibilityLabel={sshKeys.backA11y}
            accessibilityRole="button"
            onPress={() => navigation.goBack()}
            style={({ pressed }) => [
              styles.back,
              { minHeight: hit.min },
              pressed && !isAndroid() ? styles.backPressed : null,
            ]}
            testID="ssh-keys-back"
          >
            <Icon color="accent" name="chevronLeft" size={17} />
            <AppText color="accent" variant="headline" weight={600}>
              {sshKeys.backText}
            </AppText>
          </Pressable>
          <AppText accessibilityRole="header" variant="title" weight={700}>
            {sshKeys.title}
          </AppText>
        </View>

        {key !== null ? (
          <KeyCard
            item={key}
            key={key.keyId}
            onCopyFailed={() =>
              show(sshKeys.keyCard.copyFailed, { tone: 'bad' })
            }
          />
        ) : keysQuery.isLoading ? null : keysQuery.isError ? (
          // T14: a failed read is not "no key"; never offer to make another.
          <EmptyState
            action={{
              label: common.retry,
              loading: keysQuery.isFetching,
              onPress: () => {
                keysQuery.refetch().catch(() => undefined);
              },
              testID: 'ssh-keys-retry',
            }}
            icon="alert"
            message={sshKeys.loadFailed.message}
            title={sshKeys.loadFailed.title}
          />
        ) : (
          <EmptyState
            action={{
              label: sshKeys.empty.generate,
              loading: generating,
              onPress: () => {
                generate().catch(() => undefined);
              },
              testID: 'ssh-keys-generate',
            }}
            icon="key"
            message={sshKeys.empty.message}
            title={sshKeys.empty.title}
          />
        )}

        <View style={styles.section}>
          <SectionHeader title={sshKeys.install.section} />
          {installView.rows.length === 0 ? (
            <AppText color="text3" variant="caption">
              {sshKeys.install.empty}
            </AppText>
          ) : (
            <ListGroup>
              {installView.rows.map(row => {
                const installed =
                  row.installed || installedNow.has(row.serverId);
                return (
                  <ListRow
                    key={row.serverId}
                    right={
                      installed ? (
                        <StatusChip
                          label={sshKeys.install.installed}
                          tone="good"
                        />
                      ) : (
                        <Button
                          accessibilityLabel={sshKeys.install.a11y(row.name)}
                          disabled={key === null}
                          onPress={() => installAction(row.serverId)}
                          title={sshKeys.install.action}
                          variant="pill"
                        />
                      )
                    }
                    subtitle={
                      installedNow.has(row.serverId)
                        ? sshKeys.install.justInstalled
                        : row.subtitle
                    }
                    testID={`install-row-${row.serverId}`}
                    title={row.name}
                  />
                );
              })}
            </ListGroup>
          )}
          {firstTailscale === undefined ? null : (
            <AppText color="text3" variant="caption">
              {sshKeys.install.tailscaleHint(firstTailscale)}
            </AppText>
          )}
        </View>

        <View style={styles.section}>
          <SectionHeader
            right={sshKeys.trusted.hint}
            title={sshKeys.trusted.section}
          />
          {trustedCount === 0 ? null : (
            <ListGroup>
              {tailscaleServers.map(server => (
                <ListRow
                  key={`tailscale-${server.serverId}`}
                  subtitle={sshKeys.trusted.tailscaleSubtitle}
                  testID={`trusted-tailscale-${server.serverId}`}
                  title={server.name}
                  value={sshKeys.trusted.tailscaleValue}
                />
              ))}
              {trustUnsupported
                ? null
                : trustRows.map(row => (
                    <ListRow
                      key={`trusted-${row.serverId}`}
                      right={
                        <Button
                          accessibilityLabel={sshKeys.trusted.deleteA11y(
                            row.name,
                          )}
                          onPress={() =>
                            setForgetTarget({
                              serverId: row.serverId,
                              name: row.name,
                            })
                          }
                          title={common.delete}
                          variant="pillDanger"
                        />
                      }
                      subtitle={`${row.keyType} ${row.short}`}
                      testID={`trusted-row-${row.serverId}`}
                      title={row.name}
                    />
                  ))}
            </ListGroup>
          )}
          {trustUnsupported ? (
            <AppText color="text3" variant="caption">
              {sshKeys.trusted.unsupported}
            </AppText>
          ) : trustEmpty ? (
            <AppText color="text3" variant="caption">
              {sshKeys.trusted.empty}
            </AppText>
          ) : trustFailed ? (
            <AppText color="bad" variant="caption">
              {sshKeys.trusted.loadFailed}
            </AppText>
          ) : null}
        </View>
      </ScrollView>

      <InstallDialog
        installing={installing}
        onCancel={() => {
          if (!installing) {
            setInstallTarget(null);
          }
        }}
        onInstall={() => install()}
        serverName={installTarget?.name ?? ''}
        visible={installTarget !== null}
      />

      <Dialog
        actions={[
          {
            label: common.delete,
            loading: forgetting,
            onPress: () => {
              forget().catch(() => undefined);
            },
            variant: 'pillDanger',
          },
          {
            label: common.cancel,
            onPress: () => setForgetTarget(null),
            variant: 'secondary',
          },
        ]}
        message={sshKeys.forgetDialog.message}
        onDismiss={forgetting ? undefined : () => setForgetTarget(null)}
        testID="forget-dialog"
        title={sshKeys.forgetDialog.title(forgetTarget?.name ?? '')}
        visible={forgetTarget !== null}
      />

      <Toast toast={toast} onHide={hide} />
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: tokens.bg },
  content: {
    gap: space.section,
    paddingHorizontal: space.gutter,
    paddingTop: space.sm,
  },
  header: { gap: space.xs },
  backPressed: { opacity: press.opacity },
  back: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: space.xs,
  },
  section: { gap: space.sm },
});
