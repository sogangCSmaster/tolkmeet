#!/usr/bin/env bash
# 1Password의 비밀값을 서버의 /etc/tolkmeet/secrets.env로 바로 보낸다(화면·파일·로그에 남기지 않는다).
#   deploy/secrets.sh            Paddle 값은 빼고 보낸다(결제는 꺼진 채로)
#   PADDLE=1 deploy/secrets.sh   Paddle live 값도 보낸다(결제가 켜진다). 도메인 승인·계정 심사 뒤에만.
set -euo pipefail
cd "$(dirname "$0")"
. ./common.sh
if [ "${PADDLE:-}" = 1 ]; then
	tpl=$(cat secrets.env.tpl)
else
	tpl=$(grep -v '^PADDLE_' secrets.env.tpl)
fi
printf '%s\n' "$tpl" | "${OP[@]}" inject | "${SSH[@]}" "$DEPLOY_HOST" \
	'sudo install -m 0600 -o root -g root /dev/stdin /etc/tolkmeet/secrets.env && sudo systemctl try-restart tolkmeet && echo "secrets updated"'
