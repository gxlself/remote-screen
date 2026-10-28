#!/usr/bin/env bash
# Use local, gitignored build secrets by default.
# Nacos is optional: set BUILD_SECRETS_PROVIDER=nacos and the NACOS_* variables,
# or point NACOS_CONFIG_FILE at a private env file. No other checkout is needed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

ACTION="pull"
PLATFORM="all"
APPLY=0
REQUIRE_ANDROID_SIGNING=0

while (($#)); do
  case "$1" in
    pull|publish|diff)
      ACTION="$1"
      shift
      ;;
    --apply|-y|--yes)
      APPLY=1
      shift
      ;;
    --platform)
      [[ $# -ge 2 ]] || { echo '--platform requires android, ios, both, or all.' >&2; exit 1; }
      PLATFORM="$2"
      shift 2
      ;;
    --require-android-signing)
      REQUIRE_ANDROID_SIGNING=1
      shift
      ;;
    -h|--help)
      sed -n '2,4p' "$0" | sed 's/^# \{0,1\}//'
      cat <<'EOF'

Usage:
  bash scripts/nacos-build-secrets.sh pull [--platform android|ios|both|all]
  bash scripts/nacos-build-secrets.sh publish --apply
  bash scripts/nacos-build-secrets.sh diff

Env overrides:
  BUILD_SECRETS_PROVIDER=local|nacos (default: local; Nacos coordinates opt in)
  NACOS_CONFIG_FILE (only this explicitly named env file is read)
  NACOS_SERVER / NACOS_NAMESPACE / NACOS_GROUP / NACOS_DATA_ID
  NACOS_USERNAME / NACOS_PASSWORD
EOF
      exit 0
      ;;
    *)
      echo "未知参数: $1" >&2
      exit 1
      ;;
  esac
done

case "$PLATFORM" in
  android|ios|both|all) ;;
  *) echo "❌ --platform 必须是 android、ios、both 或 all，当前: $PLATFORM" >&2; exit 1 ;;
esac

source "$SCRIPT_DIR/build-secrets-config.sh"
load_build_secrets_config

if [[ "$ACTION" == publish && "$APPLY" == 0 ]]; then
  echo 'Publishing to Nacos requires explicit --apply.' >&2
  exit 1
fi

if [[ "$ACTION" == "pull" && "${BUILD_SECRETS_SYNCED:-0}" == "1" ]]; then
  echo "ℹ️  构建密钥已同步，跳过重复拉取"
  exit 0
fi

normalize_base_url() {
  local base="${NACOS_SERVER%/}"
  if [[ ! "$base" =~ ^https?:// ]]; then
    base="http://${base}"
  fi
  if [[ "$base" != */nacos ]]; then
    base="${base}/nacos"
  fi
  printf '%s\n' "$base"
}

urlencode() {
  node -e 'process.stdout.write(encodeURIComponent(process.argv[1] || ""))' "$1"
}

json_field() {
  local field="$1"
  node -e '
const fs = require("fs");
const field = process.argv[1];
let data = "";
process.stdin.on("data", chunk => data += chunk);
process.stdin.on("end", () => {
  try {
    const obj = JSON.parse(data);
    process.stdout.write(obj[field] || "");
  } catch (_) {}
});
' "$field"
}

curl_with_retries() {
  local retries="${NACOS_CURL_RETRIES:-3}"
  local delay="${NACOS_CURL_RETRY_DELAY:-2}"
  local connect_timeout="${NACOS_CURL_CONNECT_TIMEOUT:-10}"
  local max_time="${NACOS_CURL_MAX_TIME:-60}"
  local attempt=1
  local status=0

  while (( attempt <= retries )); do
    if curl --connect-timeout "$connect_timeout" --max-time "$max_time" "$@"; then
      return 0
    else
      status=$?
    fi
    if (( attempt >= retries )); then
      return "$status"
    fi

    echo "⚠️  Nacos curl 失败(exit ${status})，${delay}s 后重试 (${attempt}/${retries})" >&2
    sleep "$delay"
    attempt=$((attempt + 1))
  done

  return "$status"
}

curl_http_code_with_retries() {
  local retries="${NACOS_CURL_RETRIES:-3}"
  local delay="${NACOS_CURL_RETRY_DELAY:-2}"
  local connect_timeout="${NACOS_CURL_CONNECT_TIMEOUT:-10}"
  local max_time="${NACOS_CURL_MAX_TIME:-60}"
  local attempt=1
  local status=0
  local output=""

  while (( attempt <= retries )); do
    if output="$(curl --connect-timeout "$connect_timeout" --max-time "$max_time" "$@")"; then
      printf '%s' "$output"
      return 0
    else
      status=$?
    fi
    if (( attempt >= retries )); then
      return "$status"
    fi

    echo "⚠️  Nacos curl 失败(exit ${status})，${delay}s 后重试 (${attempt}/${retries})" >&2
    sleep "$delay"
    attempt=$((attempt + 1))
  done

  return "$status"
}

nacos_login() {
  if [[ -z "${NACOS_USERNAME:-}" && -z "${NACOS_PASSWORD:-}" ]]; then
    echo "ℹ️  未配置 Nacos 账号密码，尝试匿名访问"
    return
  fi

  local resp
  resp="$(curl_with_retries -sS -X POST "${BASE_URL}/v1/auth/login" \
    --data-urlencode "username=${NACOS_USERNAME:-}" \
    --data-urlencode "password=${NACOS_PASSWORD:-}" || true)"
  TOKEN="$(printf '%s' "$resp" | json_field accessToken)"

  if [[ -n "$TOKEN" ]]; then
    echo "✅ Nacos 登录成功"
    return
  fi

  case "$resp" in
    *"error"*|*"unknown user"*|*"invalid"*|*"403"*|*"401"*)
      echo "❌ Nacos 登录失败，请检查 NACOS_USERNAME/NACOS_PASSWORD" >&2
      exit 1
      ;;
  esac

  echo "ℹ️  Nacos 未返回 token，继续尝试读取配置"
}

fetch_remote() {
  local out="$1"
  local args=(
    -sS -o "$out" -w '%{http_code}' -G "${BASE_URL}/v1/cs/configs"
    --data-urlencode "dataId=${NACOS_DATA_ID}"
    --data-urlencode "group=${NACOS_GROUP}"
    --data-urlencode "tenant=${NACOS_NAMESPACE}"
  )
  if [[ -n "$TOKEN" ]]; then
    args+=(--data-urlencode "accessToken=${TOKEN}")
  fi
  curl_http_code_with_retries "${args[@]}"
}

publish_remote() {
  local file="$1"
  local resp
  local http
  local url="${BASE_URL}/v1/cs/configs"
  resp="$(mktemp)"

  if [[ -n "$TOKEN" ]]; then
    url="${url}?accessToken=$(urlencode "$TOKEN")"
  fi

  http="$(curl_http_code_with_retries -sS -o "$resp" -w '%{http_code}' -X POST "$url" \
    --data-urlencode "dataId=${NACOS_DATA_ID}" \
    --data-urlencode "group=${NACOS_GROUP}" \
    --data-urlencode "tenant=${NACOS_NAMESPACE}" \
    --data-urlencode "type=properties" \
    --data-urlencode "content@${file}" || true)"

  if [[ "$http" == "200" && "$(cat "$resp")" == "true" ]]; then
    rm -f "$resp"
    return 0
  fi

  echo "❌ Nacos 发布失败 HTTP=$http" >&2
  rm -f "$resp"
  return 1
}

write_local_secret_files() {
  local content_file="$1"
  ROOT_DIR="$ROOT_DIR" \
  CONTENT_FILE="$content_file" \
  SECRETS_PLATFORM="$PLATFORM" \
  REQUIRE_ANDROID_SIGNING="$REQUIRE_ANDROID_SIGNING" \
  node <<'NODE'
const fs = require('fs');
const path = require('path');

const root = process.env.ROOT_DIR;
const platform = process.env.SECRETS_PLATFORM || 'all';
const requireAndroidSigning = process.env.REQUIRE_ANDROID_SIGNING === '1';
const input = fs.readFileSync(process.env.CONTENT_FILE, 'utf8');

function parseProperties(text) {
  const props = {};
  for (const rawLine of text.split(/\r?\n/)) {
    const trimmed = rawLine.trim();
    if (!trimmed || trimmed.startsWith('#') || trimmed.startsWith('!')) continue;

    let split = -1;
    let escaped = false;
    for (let i = 0; i < rawLine.length; i += 1) {
      const ch = rawLine[i];
      if (escaped) {
        escaped = false;
        continue;
      }
      if (ch === '\\') {
        escaped = true;
        continue;
      }
      if (ch === '=' || ch === ':') {
        split = i;
        break;
      }
    }

    let key;
    let value;
    if (split >= 0) {
      key = rawLine.slice(0, split).trim();
      value = rawLine.slice(split + 1).trim();
    } else {
      const match = rawLine.match(/^(\S+)\s+(.*)$/);
      if (!match) continue;
      key = match[1].trim();
      value = match[2].trim();
    }
    props[key] = value.replace(/^['"]|['"]$/g, '');
  }
  return props;
}

const props = parseProperties(input);
const written = [];
const warnings = [];
const errors = [];

function wantAndroid() {
  return platform === 'android' || platform === 'both' || platform === 'all';
}

function wantIos() {
  return platform === 'ios' || platform === 'both' || platform === 'all';
}

function pick(...keys) {
  for (const key of keys) {
    const value = props[key];
    if (value !== undefined && String(value).trim() !== '') return String(value).trim();
  }
  return '';
}

function abs(rel) {
  return path.isAbsolute(rel) ? rel : path.join(root, rel);
}

function ensureDir(file) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
}

function writeText(rel, content) {
  const file = abs(rel);
  ensureDir(file);
  fs.writeFileSync(file, content, { mode: 0o600 });
  fs.chmodSync(file, 0o600);
  written.push(rel);
}

function writeBase64(rel, ...keys) {
  const value = pick(...keys);
  if (!value) return false;
  const file = abs(rel);
  ensureDir(file);
  fs.writeFileSync(file, Buffer.from(value, 'base64'), { mode: 0o600 });
  fs.chmodSync(file, 0o600);
  written.push(rel);
  return true;
}

if (wantAndroid()) {
  // 阿里号码认证(一键登录)是可选能力：有就写出，没有仅告警，不阻断构建。
  // 需要强制校验的 app 可自行把 warnings 改成 errors。
  const androidAuth = pick('ANDROID_ALIYUN_PNVS_AUTH_SECRET', 'ALIYUN_PNVS_AUTH_SECRET');
  if (androidAuth) {
    writeText('android/aliyun-auth.properties', `ALIYUN_PNVS_AUTH_SECRET=${androidAuth}\n`);
  } else {
    warnings.push('Nacos 未提供 ANDROID_ALIYUN_PNVS_AUTH_SECRET（无一键登录则可忽略）');
  }

  const pushKeys = [
    'ALI_APP_KEY',
    'ALI_APP_SECRET',
    'XIAOMI_APP_ID',
    'XIAOMI_APP_KEY',
    'OPPO_APP_KEY',
    'OPPO_APP_SECRET',
    'VIVO_APP_ID',
    'VIVO_API_KEY',
    'HONOR_APP_ID',
  ];
  const hasPush = pushKeys.some(key => pick(key));
  if (hasPush) {
    const lines = [
      '# Generated from Nacos. Do not commit.',
      ...pushKeys.map(key => `${key}=${pick(key)}`),
      '',
    ];
    writeText('android/push.properties', lines.join('\n'));
  }

  const storeFile = pick('MYAPP_UPLOAD_STORE_FILE', 'ANDROID_KEYSTORE_FILE');
  const storeAlias = pick('MYAPP_UPLOAD_KEY_ALIAS');
  const storePassword = pick('MYAPP_UPLOAD_STORE_PASSWORD');
  const keyPassword = pick('MYAPP_UPLOAD_KEY_PASSWORD');
  if (storeFile || storeAlias || storePassword || keyPassword) {
    if (storeFile && storeAlias && storePassword && keyPassword) {
      writeText('android/keystore.local.properties', [
        '# Generated from Nacos. Do not commit.',
        `MYAPP_UPLOAD_STORE_FILE=${storeFile}`,
        `MYAPP_UPLOAD_KEY_ALIAS=${storeAlias}`,
        `MYAPP_UPLOAD_STORE_PASSWORD=${storePassword}`,
        `MYAPP_UPLOAD_KEY_PASSWORD=${keyPassword}`,
        '',
      ].join('\n'));
    } else {
      errors.push('complete Android signing properties');
    }
  }

  const keystoreRel = storeFile ? (path.isAbsolute(storeFile) ? storeFile : path.posix.join('android', storeFile)) : 'android/app/release.keystore';
  const wroteKeystore = writeBase64(
    keystoreRel,
    'ANDROID_RELEASE_KEYSTORE_BASE64',
    'ANDROID_KEYSTORE_BASE64',
  );
  if (requireAndroidSigning) {
    const hasProps = fs.existsSync(abs('android/keystore.local.properties')) || fs.existsSync(abs('android/keystore.properties'));
    const hasKeystore = fs.existsSync(abs(keystoreRel));
    if (!hasProps) errors.push('android/keystore.local.properties');
    if (!hasKeystore && !wroteKeystore) errors.push(keystoreRel);
  }

  writeBase64('android/app/agconnect-services.json', 'ANDROID_AGCONNECT_SERVICES_JSON_BASE64');
  writeBase64('android/app/google-services.json', 'ANDROID_GOOGLE_SERVICES_JSON_BASE64');
  writeBase64('android/app/aliyun-emas-services.json', 'ANDROID_ALIYUN_EMAS_SERVICES_JSON_BASE64');
}

if (wantIos()) {
  const iosAuth = pick('IOS_ALIYUN_ONE_KEY_AUTH_SECRET', 'ALIYUN_ONE_KEY_AUTH_SECRET');
  if (iosAuth) {
    writeText('ios/aliyun-auth.xcconfig', `ALIYUN_ONE_KEY_AUTH_SECRET = ${iosAuth}\n`);
  } else {
    warnings.push('Nacos 未提供 IOS_ALIYUN_ONE_KEY_AUTH_SECRET（无一键登录则可忽略）');
  }

  writeBase64('ios/AliyunEmasServices-Info.plist', 'IOS_ALIYUN_EMAS_SERVICES_INFO_PLIST_BASE64');

  // App Store Connect API key（TestFlight 上传鉴权）：.p8 + Key ID + Issuer ID
  const ascKeyId = pick('IOS_ASC_API_KEY_ID');
  const ascIssuerId = pick('IOS_ASC_API_ISSUER_ID');
  const ascP8 = pick('IOS_ASC_API_KEY_P8_BASE64');
  if (ascP8 && ascKeyId) {
    const p8file = abs(`AuthKey_${ascKeyId}.p8`);
    ensureDir(p8file);
    fs.writeFileSync(p8file, Buffer.from(ascP8, 'base64'), { mode: 0o600 });
    fs.chmodSync(p8file, 0o600);
    written.push(`AuthKey_${ascKeyId}.p8`);
  }
  if (ascKeyId || ascIssuerId) {
    writeText('ios/asc-api-key.properties', [
      '# Generated from Nacos. Do not commit.',
      `ASC_API_KEY_ID=${ascKeyId}`,
      `ASC_API_ISSUER_ID=${ascIssuerId}`,
      '',
    ].join('\n'));
  }
}

if (written.length) {
  console.log(`✅ 已从 Nacos 生成 ${written.length} 个本地构建密钥文件`);
  for (const rel of written) console.log(`  - ${rel}`);
}

if (warnings.length) {
  for (const warning of warnings) console.warn(`⚠️  ${warning}`);
}

if (errors.length) {
  console.error(`❌ Nacos 构建密钥缺少: ${[...new Set(errors)].join(', ')}`);
  process.exit(1);
}
NODE
}

use_local_secret_files_or_fail() {
  local reason="$1"
  local status=0

  ROOT_DIR="$ROOT_DIR" \
  SECRETS_PLATFORM="$PLATFORM" \
  REQUIRE_ANDROID_SIGNING="$REQUIRE_ANDROID_SIGNING" \
  node <<'NODE' || status=$?
const fs = require('fs');
const path = require('path');

const root = process.env.ROOT_DIR;
const platform = process.env.SECRETS_PLATFORM || 'all';
const requireAndroidSigning = process.env.REQUIRE_ANDROID_SIGNING === '1';
const missing = [];
const found = [];

function wantAndroid() {
  return platform === 'android' || platform === 'both' || platform === 'all';
}

function wantIos() {
  return platform === 'ios' || platform === 'both' || platform === 'all';
}

function abs(rel) {
  return path.isAbsolute(rel) ? rel : path.join(root, rel);
}

function exists(rel) {
  if (!fs.existsSync(abs(rel))) return false;
  found.push(rel);
  return true;
}

function parseProperties(rel) {
  const file = abs(rel);
  if (!fs.existsSync(file)) return {};
  const props = {};
  for (const rawLine of fs.readFileSync(file, 'utf8').split(/\r?\n/)) {
    const trimmed = rawLine.trim();
    if (!trimmed || trimmed.startsWith('#') || trimmed.startsWith('!')) continue;
    const idx = rawLine.search(/[=:]/);
    if (idx < 0) continue;
    props[rawLine.slice(0, idx).trim()] = rawLine.slice(idx + 1).trim();
  }
  return props;
}

if (wantAndroid()) {
  exists('android/aliyun-auth.properties');

  if (requireAndroidSigning) {
    const propsRel = fs.existsSync(abs('android/keystore.local.properties'))
      ? 'android/keystore.local.properties'
      : 'android/keystore.properties';
    if (!exists(propsRel)) {
      missing.push('android/keystore.local.properties 或 android/keystore.properties');
    }

    const signingProps = parseProperties(propsRel);
    for (const key of ['MYAPP_UPLOAD_STORE_FILE', 'MYAPP_UPLOAD_KEY_ALIAS', 'MYAPP_UPLOAD_STORE_PASSWORD', 'MYAPP_UPLOAD_KEY_PASSWORD']) {
      if (!signingProps[key]) missing.push(key);
    }
    const storeFile = signingProps.MYAPP_UPLOAD_STORE_FILE || 'app/release.keystore';
    const keystoreRel = path.isAbsolute(storeFile) ? storeFile : path.posix.join('android', storeFile);
    if (!exists(keystoreRel)) {
      missing.push(keystoreRel);
    }
  }
}

if (wantIos()) {
  exists('ios/aliyun-auth.xcconfig');
  exists('ios/asc-api-key.properties');
}

if (missing.length) {
  console.error(`❌ 本地构建密钥缺失: ${[...new Set(missing)].join(', ')}`);
  process.exit(1);
}

console.log(`✅ 使用本地构建密钥继续构建`);
for (const rel of [...new Set(found)]) console.log(`  - ${rel}`);
NODE
  if (( status == 0 )); then
    echo "ℹ️  使用本地密钥：${reason}"
    exit 0
  fi

  echo "请准备本地构建密钥，或配置可选的 Nacos provider 后重试" >&2
  exit "$status"
}

collect_local_secret_files() {
  local out="$1"
  ROOT_DIR="$ROOT_DIR" OUT_FILE="$out" node <<'NODE'
const fs = require('fs');
const path = require('path');

const root = process.env.ROOT_DIR;
const outFile = process.env.OUT_FILE;
const lines = [
  '# Generated by scripts/nacos-build-secrets.sh publish.',
  '# Do not paste this file into Git.',
  '',
];
const added = [];

function abs(rel) {
  return path.isAbsolute(rel) ? rel : path.join(root, rel);
}

function parseProperties(text) {
  const props = {};
  for (const rawLine of text.split(/\r?\n/)) {
    const trimmed = rawLine.trim();
    if (!trimmed || trimmed.startsWith('#') || trimmed.startsWith('!')) continue;
    const idx = rawLine.search(/[=:]/);
    if (idx < 0) continue;
    const key = rawLine.slice(0, idx).trim();
    const value = rawLine.slice(idx + 1).trim();
    props[key] = value;
  }
  return props;
}

function add(key, value) {
  if (value === undefined || value === null || String(value).trim() === '') return;
  lines.push(`${key}=${String(value).trim()}`);
  added.push(key);
}

function addProps(rel, keys, rename = {}) {
  const file = abs(rel);
  if (!fs.existsSync(file)) return {};
  const props = parseProperties(fs.readFileSync(file, 'utf8'));
  for (const key of keys) add(rename[key] || key, props[key]);
  return props;
}

function addBase64(rel, key) {
  const file = abs(rel);
  if (!fs.existsSync(file)) return false;
  add(key, fs.readFileSync(file).toString('base64'));
  return true;
}

addProps('android/aliyun-auth.properties', ['ALIYUN_PNVS_AUTH_SECRET'], {
  ALIYUN_PNVS_AUTH_SECRET: 'ANDROID_ALIYUN_PNVS_AUTH_SECRET',
});

addProps('android/push.properties', [
  'ALI_APP_KEY',
  'ALI_APP_SECRET',
  'XIAOMI_APP_ID',
  'XIAOMI_APP_KEY',
  'OPPO_APP_KEY',
  'OPPO_APP_SECRET',
  'VIVO_APP_ID',
  'VIVO_API_KEY',
  'HONOR_APP_ID',
]);

let signingProps = addProps('android/keystore.local.properties', [
  'MYAPP_UPLOAD_STORE_FILE',
  'MYAPP_UPLOAD_KEY_ALIAS',
  'MYAPP_UPLOAD_STORE_PASSWORD',
  'MYAPP_UPLOAD_KEY_PASSWORD',
]);
if (!Object.keys(signingProps).length) {
  signingProps = addProps('android/keystore.properties', [
    'MYAPP_UPLOAD_STORE_FILE',
    'MYAPP_UPLOAD_KEY_ALIAS',
    'MYAPP_UPLOAD_STORE_PASSWORD',
    'MYAPP_UPLOAD_KEY_PASSWORD',
  ]);
}
const storeFile = signingProps.MYAPP_UPLOAD_STORE_FILE || 'app/release.keystore';
addBase64(path.isAbsolute(storeFile) ? storeFile : path.posix.join('android', storeFile), 'ANDROID_RELEASE_KEYSTORE_BASE64');

addBase64('android/app/agconnect-services.json', 'ANDROID_AGCONNECT_SERVICES_JSON_BASE64');
addBase64('android/app/google-services.json', 'ANDROID_GOOGLE_SERVICES_JSON_BASE64');
addBase64('android/app/aliyun-emas-services.json', 'ANDROID_ALIYUN_EMAS_SERVICES_JSON_BASE64');

addProps('ios/aliyun-auth.xcconfig', ['ALIYUN_ONE_KEY_AUTH_SECRET'], {
  ALIYUN_ONE_KEY_AUTH_SECRET: 'IOS_ALIYUN_ONE_KEY_AUTH_SECRET',
});
addBase64('ios/AliyunEmasServices-Info.plist', 'IOS_ALIYUN_EMAS_SERVICES_INFO_PLIST_BASE64');

// App Store Connect API key（TestFlight 上传鉴权）：.p8 + Key ID + Issuer ID
const ascProps = addProps('ios/asc-api-key.properties', ['ASC_API_KEY_ID', 'ASC_API_ISSUER_ID'], {
  ASC_API_KEY_ID: 'IOS_ASC_API_KEY_ID',
  ASC_API_ISSUER_ID: 'IOS_ASC_API_ISSUER_ID',
});
let ascKeyId = ascProps.ASC_API_KEY_ID;
if (!ascKeyId) {
  const found = fs.readdirSync(root).find(f => /^AuthKey_.+\.p8$/.test(f));
  if (found) ascKeyId = found.replace(/^AuthKey_(.+)\.p8$/, '$1');
}
if (ascKeyId) {
  addBase64(`AuthKey_${ascKeyId}.p8`, 'IOS_ASC_API_KEY_P8_BASE64');
}

lines.push('');
fs.writeFileSync(outFile, lines.join('\n'), { mode: 0o600 });
fs.chmodSync(outFile, 0o600);

if (!added.length) {
  console.error('❌ 没有找到可发布的本地密钥文件');
  process.exit(1);
}
console.log(`✅ 已收集 ${added.length} 个密钥字段`);
NODE
}

if [[ "$BUILD_SECRETS_PROVIDER" == local ]]; then
  if [[ "$ACTION" != pull ]]; then
    echo 'publish/diff require BUILD_SECRETS_PROVIDER=nacos and Nacos coordinates.' >&2
    exit 1
  fi
  use_local_secret_files_or_fail 'local provider'
fi
BASE_URL="$(normalize_base_url)"
TOKEN=""

echo "▶️  Nacos: ${BASE_URL}  ns=${NACOS_NAMESPACE}  group=${NACOS_GROUP}  dataId=${NACOS_DATA_ID}"
nacos_login

case "$ACTION" in
  pull)
    tmp="$(mktemp)"
    code="$(fetch_remote "$tmp" || true)"
    if [[ -z "$code" ]]; then
      echo "❌ 拉取 Nacos 构建密钥失败：网络请求失败" >&2
      rm -f "$tmp"
      use_local_secret_files_or_fail "网络请求失败"
    fi
    if [[ "$code" != "200" ]]; then
      echo "❌ 拉取 Nacos 构建密钥失败 HTTP=$code" >&2
      echo "请确认 group=${NACOS_GROUP}、dataId=${NACOS_DATA_ID} 已发布" >&2
      rm -f "$tmp"
      use_local_secret_files_or_fail "HTTP=$code"
    fi
    write_local_secret_files "$tmp"
    rm -f "$tmp"
    ;;
  diff)
    tmp="$(mktemp)"
    code="$(fetch_remote "$tmp" || true)"
    if [[ -z "$code" ]]; then
      echo "ℹ️  Nacos 网络请求失败，无法对比远端配置"
      rm -f "$tmp"
      exit 0
    fi
    if [[ "$code" != "200" ]]; then
      echo "ℹ️  远端无配置或无权限 HTTP=$code"
      rm -f "$tmp"
      exit 0
    fi
    local_tmp="$(mktemp)"
    collect_local_secret_files "$local_tmp"
    REMOTE_FILE="$tmp" LOCAL_FILE="$local_tmp" node <<'NODE'
const fs = require('fs');

function parse(file) {
  const props = {};
  for (const rawLine of fs.readFileSync(file, 'utf8').split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith('#') || line.startsWith('!')) continue;
    const idx = rawLine.search(/[=:]/);
    if (idx < 0) continue;
    props[rawLine.slice(0, idx).trim()] = rawLine.slice(idx + 1).trim();
  }
  return props;
}

const remote = parse(process.env.REMOTE_FILE);
const local = parse(process.env.LOCAL_FILE);
const remoteKeys = new Set(Object.keys(remote));
const localKeys = new Set(Object.keys(local));
const onlyRemote = [...remoteKeys].filter(key => !localKeys.has(key)).sort();
const onlyLocal = [...localKeys].filter(key => !remoteKeys.has(key)).sort();
const changed = [...localKeys].filter(key => remoteKeys.has(key) && remote[key] !== local[key]).sort();

if (!onlyRemote.length && !onlyLocal.length && !changed.length) {
  console.log('✅ Nacos 构建密钥与本地一致');
} else {
  if (onlyRemote.length) console.log(`远端独有 key: ${onlyRemote.join(', ')}`);
  if (onlyLocal.length) console.log(`本地独有 key: ${onlyLocal.join(', ')}`);
  if (changed.length) console.log(`值不同 key: ${changed.join(', ')}`);
}
NODE
    rm -f "$tmp" "$local_tmp"
    ;;
  publish)
    if (( ! APPLY )); then
      echo "❌ 发布到 Nacos 需要显式加 --apply" >&2
      exit 1
    fi
    tmp="$(mktemp)"
    collect_local_secret_files "$tmp"
    publish_remote "$tmp"
    rm -f "$tmp"
    echo "✅ 构建密钥已发布到 Nacos"
    ;;
esac
