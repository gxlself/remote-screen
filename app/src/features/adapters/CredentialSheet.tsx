import { useRef, useState } from 'react';
import type { JSX } from 'react';
import { Pressable, StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Banner } from '../../components/Banner';
import BottomSheet from '../../components/BottomSheet';
import { Button } from '../../components/Button';
import { Switch } from '../../components/Switch';
import { TextField } from '../../components/TextField';
import type { TextFieldHandle } from '../../components/TextField';
import { adapters } from '../../i18n/zh/adapters';
import type {
  AdapterCredField,
  AdapterProtocol,
} from '../../native/rscore/types';
import { space } from '../../theme/tokens';
import {
  CRED_LIMITS,
  validateCredentials,
  type CredFieldError,
  type CredInput,
} from './logic';

export type CredentialSheetProps = {
  protocol: AdapterProtocol;
  fields: readonly AdapterCredField[];
  /** 上一次认证失败（adapter.state 的 credError）。 */
  failed: boolean;
  /** adapters.canSave()；false 时不显示「记住」开关，提交的 save 恒为 false。 */
  canSave: boolean;
  /** 上次输入或已保存的用户名 / 域，只用来预填。 */
  defaultUser?: string;
  defaultDomain?: string;
  /** 返回 false 表示没交给适配器（会话已断等），弹层提示后可以重试。 */
  onSubmit: (v: CredInput, save: boolean) => boolean;
  onCancel: () => void;
};

type CredErrors = Partial<Record<AdapterCredField, CredFieldError>>;

function errorText(field: AdapterCredField, error: CredFieldError): string {
  switch (error) {
    case 'empty':
      return adapters.creds.invalidEmpty;
    case 'length':
      return adapters.creds.invalidLength(CRED_LIMITS[field]);
    case 'control':
      return adapters.creds.invalidControl;
  }
}

export function CredentialSheet(p: CredentialSheetProps): JSX.Element {
  const [values, setValues] = useState<CredInput>(() => ({
    user: p.defaultUser ?? '',
    password: '',
    domain: p.defaultDomain ?? '',
  }));
  const [errors, setErrors] = useState<CredErrors>({});
  const [remember, setRemember] = useState(true);
  // Stays on until the adapter moves on and this sheet unmounts.
  const [submitting, setSubmitting] = useState(false);
  const [submitFailed, setSubmitFailed] = useState(false);
  const passwordOnly = !p.fields.includes('user');
  const hasDomain = p.fields.includes('domain');
  const passwordRef = useRef<TextFieldHandle>(null);
  const domainRef = useRef<TextFieldHandle>(null);
  // Kept from a failed attempt or saved: start on the password (V14).
  const [focusPassword] = useState(
    () => passwordOnly || (p.defaultUser ?? '') !== '',
  );

  const change = (field: keyof CredInput, text: string) => {
    setValues(current => ({ ...current, [field]: text }));
    setErrors(current => {
      if (current[field] === undefined) {
        return current;
      }
      const next = { ...current };
      delete next[field];
      return next;
    });
  };

  const submit = () => {
    if (submitting) {
      return;
    }
    const found = validateCredentials(p.fields, values);
    if (Object.keys(found).length > 0) {
      setErrors(found);
      return;
    }
    setErrors({});
    let sent = false;
    try {
      sent = p.onSubmit({ ...values }, p.canSave && remember);
    } finally {
      setValues(current => ({ ...current, password: '' }));
      setSubmitting(sent);
      setSubmitFailed(!sent);
    }
  };

  const userError = errors.user;
  const passwordError = errors.password;
  const domainError = errors.domain;

  return (
    <View accessibilityLabel={adapters.a11y.credsSheet} style={styles.host}>
      <BottomSheet
        dismissible={!submitting}
        footer={
          <View style={styles.actions}>
            <Button
              fullWidth
              onPress={p.onCancel}
              title={adapters.creds.cancel}
              variant="secondary"
              testID="adapter-creds-cancel"
            />
            <Button
              fullWidth
              loading={submitting}
              onPress={submit}
              title={adapters.creds.submit}
              testID="adapter-creds-submit"
            />
          </View>
        }
        onClose={p.onCancel}
        subtitle={adapters.creds.intro(p.protocol, passwordOnly)}
        testID="adapter-creds"
        title={adapters.creds.title(p.protocol)}
      >
        <View style={styles.content}>
          {submitFailed ? (
            <Banner
              tone="bad"
              title={adapters.creds.submitFailed}
              testID="adapter-creds-submit-failed"
            />
          ) : p.failed ? (
            <Banner
              tone="bad"
              title={adapters.creds.failed(passwordOnly)}
              testID="adapter-creds-failed"
            />
          ) : null}
          {p.fields.includes('user') ? (
            <TextField
              autoCapitalize="none"
              autoCorrect={false}
              error={
                userError === undefined
                  ? undefined
                  : errorText('user', userError)
              }
              autoComplete="username"
              autoFocus={!focusPassword}
              blurOnSubmit={false}
              label={adapters.creds.user}
              onChangeText={text => change('user', text)}
              onSubmitEditing={() => passwordRef.current?.focus()}
              returnKeyType="next"
              testID="adapter-creds-user"
              textContentType="username"
              value={values.user}
            />
          ) : null}
          {p.fields.includes('password') ? (
            <TextField
              error={
                passwordError === undefined
                  ? undefined
                  : errorText('password', passwordError)
              }
              autoComplete="current-password"
              autoFocus={focusPassword}
              blurOnSubmit={!hasDomain}
              label={adapters.creds.password}
              onChangeText={text => change('password', text)}
              onSubmitEditing={() =>
                hasDomain ? domainRef.current?.focus() : submit()
              }
              ref={passwordRef}
              returnKeyType={hasDomain ? 'next' : 'go'}
              secure
              testID="adapter-creds-password"
              textContentType="password"
              value={values.password}
            />
          ) : null}
          {hasDomain ? (
            <TextField
              autoCapitalize="none"
              autoCorrect={false}
              error={
                domainError === undefined
                  ? undefined
                  : errorText('domain', domainError)
              }
              label={adapters.creds.domain}
              onChangeText={text => change('domain', text)}
              onSubmitEditing={submit}
              ref={domainRef}
              returnKeyType="go"
              testID="adapter-creds-domain"
              value={values.domain}
            />
          ) : null}
          {p.canSave ? (
            <Pressable
              accessible={false}
              onPress={() => setRemember(value => !value)}
              style={styles.remember}
              testID="adapter-creds-remember-row"
            >
              <AppText color="text" variant="body">
                {adapters.creds.remember}
              </AppText>
              <Switch
                accessibilityLabel={adapters.creds.remember}
                onValueChange={setRemember}
                testID="adapter-creds-remember"
                value={remember}
              />
            </Pressable>
          ) : null}
        </View>
      </BottomSheet>
    </View>
  );
}

const styles = StyleSheet.create({
  host: {
    ...StyleSheet.absoluteFill,
  },
  content: {
    gap: space.md,
  },
  remember: {
    minHeight: 44,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: space.md,
  },
  actions: {
    gap: space.sm,
  },
});

export default CredentialSheet;
