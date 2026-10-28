#!/usr/bin/env bash
# Shared configuration for optional build-secret providers. Source this file,
# then call load_build_secrets_config; no config file is evaluated as shell code.

load_build_secrets_config() {
  local key value line tail quote
  if [[ "${BUILD_SECRETS_PROVIDER:-}" == local ]]; then
    export BUILD_SECRETS_PROVIDER
    return 0
  fi
  if [[ -n "${NACOS_CONFIG_FILE:-}" ]]; then
    if [[ ! -f "$NACOS_CONFIG_FILE" || ! -r "$NACOS_CONFIG_FILE" ]]; then
      echo 'Build secrets: NACOS_CONFIG_FILE does not name a readable file.' >&2
      return 1
    fi
    while IFS= read -r line || [[ -n "$line" ]]; do
      [[ "$line" == *=* ]] || continue
      key="${line%%=*}"
      key="${key#"${key%%[![:space:]]*}"}"
      key="${key%"${key##*[![:space:]]}"}"
      case "$key" in
        NACOS_SERVER|NACOS_NAMESPACE|NACOS_GROUP|NACOS_DATA_ID|NACOS_USERNAME|NACOS_PASSWORD|BUILD_SECRETS_PROVIDER|NACOS_CURL_RETRIES|NACOS_CURL_RETRY_DELAY|NACOS_CURL_CONNECT_TIMEOUT|NACOS_CURL_MAX_TIME) ;;
        *) continue ;;
      esac
      # Even an explicitly empty environment variable overrides the file.
      [[ -z "${!key+x}" ]] || continue
      value="${line#*=}"
      value="${value#"${value%%[![:space:]]*}"}"
      quote="${value:0:1}"
      if [[ "$quote" == \" || "$quote" == \' ]]; then
        value="${value:1}"
        if [[ "$value" != *"$quote"* ]]; then
          echo "Build secrets: unterminated quoted value for $key." >&2
          return 1
        fi
        tail="${value#*"$quote"}"
        value="${value%%"$quote"*}"
        tail="${tail#"${tail%%[![:space:]]*}"}"
        if [[ -n "$tail" && "$tail" != \#* ]]; then
          echo "Build secrets: unexpected text after $key." >&2
          return 1
        fi
      else
        value="${value%%[[:space:]]\#*}"
        value="${value%"${value##*[![:space:]]}"}"
      fi
      export "$key=$value"
    done < "$NACOS_CONFIG_FILE"
  fi

  # A named config file or any Nacos coordinate requests the optional provider.
  # BUILD_SECRETS_PROVIDER=local explicitly opts out even in that environment.
  if [[ -z "${BUILD_SECRETS_PROVIDER:-}" ]]; then
    if [[ -n "${NACOS_CONFIG_FILE:-}${NACOS_SERVER:-}${NACOS_NAMESPACE:-}${NACOS_GROUP:-}${NACOS_DATA_ID:-}${NACOS_USERNAME:-}${NACOS_PASSWORD:-}" ]]; then
      BUILD_SECRETS_PROVIDER=nacos
    else
      BUILD_SECRETS_PROVIDER=local
    fi
  fi
  case "$BUILD_SECRETS_PROVIDER" in
    local) ;;
    nacos)
      local missing=()
      for key in NACOS_SERVER NACOS_NAMESPACE NACOS_GROUP NACOS_DATA_ID; do
        [[ -n "${!key:-}" ]] || missing+=("$key")
      done
      if ((${#missing[@]})); then
        echo "Build secrets: Nacos requested; missing ${missing[*]}. Set environment variables or NACOS_CONFIG_FILE." >&2
        return 1
      fi
      if [[ -n "${NACOS_USERNAME:-}" && -z "${NACOS_PASSWORD:-}" || -z "${NACOS_USERNAME:-}" && -n "${NACOS_PASSWORD:-}" ]]; then
        echo 'Build secrets: set both NACOS_USERNAME and NACOS_PASSWORD, or leave both empty for anonymous access.' >&2
        return 1
      fi
      ;;
    *)
      echo 'Build secrets: BUILD_SECRETS_PROVIDER must be local or nacos.' >&2
      return 1
      ;;
  esac
  export BUILD_SECRETS_PROVIDER
}
