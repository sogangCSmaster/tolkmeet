#!/usr/bin/env bash
# 1Password의 비밀값을 서버의 /etc/tolkmeet/secrets.env로 바로 보낸다(화면·파일·로그에 남기지 않는다).
set -euo pipefail
cd "$(dirname "$0")"
. ./common.sh
"${OP[@]}" inject -i secrets.env.tpl | "${SSH[@]}" "$DEPLOY_HOST" \
	'sudo install -m 0600 -o root -g root /dev/stdin /etc/tolkmeet/secrets.env && sudo systemctl try-restart tolkmeet && echo "secrets updated"'
