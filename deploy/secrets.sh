#!/usr/bin/env bash
# 1Password의 비밀값을 서버의 /etc/tolkmeet/secrets.env로 바로 보낸다(화면·파일·로그에 남기지 않는다).
#   deploy/secrets.sh            Paddle 값은 빼고 보낸다(결제는 꺼진 채로)
#   PADDLE=1 deploy/secrets.sh   Paddle live 값도 보낸다(결제가 켜진다). 도메인 승인·계정 심사 뒤에만.
# 값을 모두 읽은 뒤에만 보낸다. 1Password 읽기가 실패하거나 빠진 값이 있으면 서버 파일을 건드리지 않고 멈춘다
# (예전에는 읽기에 실패해도 빈 내용을 그대로 써서 서버의 비밀값이 모두 지워졌다).
set -euo pipefail
cd "$(dirname "$0")"
. ./common.sh
if [ "${PADDLE:-}" = 1 ]; then
	tpl=$(cat secrets.env.tpl)
else
	tpl=$(grep -v '^PADDLE_' secrets.env.tpl)
fi
if ! out=$(printf '%s\n' "$tpl" | "${OP[@]}" inject); then
	echo "1Password에서 값을 읽지 못했습니다(서버는 그대로). 'op signin' 또는 1Password 앱 잠금 해제 후 다시 실행하세요." >&2
	exit 1
fi
# 템플릿의 변수가 모두 값과 함께 나왔는지 확인한다(이름만 비교하고 값은 출력하지 않는다).
want=$(printf '%s\n' "$tpl" | grep -oE '^[A-Z_]+=' | sort)
got=$(printf '%s\n' "$out" | grep -oE '^[A-Z_]+=.' | sed 's/.$//' | sort)
if [ -z "$want" ] || [ "$want" != "$got" ]; then
	echo "비어 있거나 빠진 값이 있습니다(서버는 그대로): $(comm -23 <(echo "$want") <(echo "$got") | tr '\n' ' ')" >&2
	exit 1
fi
printf '%s\n' "$out" | "${SSH[@]}" "$DEPLOY_HOST" \
	'sudo install -m 0600 -o root -g root /dev/stdin /etc/tolkmeet/secrets.env && sudo systemctl try-restart tolkmeet && echo "secrets updated"'
unset out
