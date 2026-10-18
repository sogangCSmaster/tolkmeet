#!/usr/bin/env bash
# 서버가 통째로 죽었을 때(상태 점검 실패 알림): 가장 최근 자동 스냅샷으로 새 서버를 만들고, 방화벽을 맞추고,
# 고정 IP를 새 서버로 옮긴 뒤, DB를 S3 백업(Litestream)의 최신 상태로 되돌린다. 옛 서버는 지우지 않고 남겨 둔다.
#   deploy/restore-from-snapshot.sh
# 필요: aws login --profile tolkmeet, 1Password SSH 키(tolkmeet-deploy)
set -euo pipefail
cd "$(dirname "$0")"
. ./common.sh
AWS=(aws --profile "${AWS_PROFILE:-tolkmeet}" --region ap-northeast-2)
OLD=${SOURCE_INSTANCE:-tolkmeet-app}
NEW="tolkmeet-app-$(date -u +%Y%m%d%H%M)"
IP=tolkmeet-ip

echo "creating $NEW from the latest automatic snapshot of $OLD"
"${AWS[@]}" lightsail create-instances-from-snapshot --instance-names "$NEW" --availability-zone ap-northeast-2a \
	--source-instance-name "$OLD" --use-latest-restorable-auto-snapshot --bundle-id micro_3_0 \
	--key-pair-name tolkmeet-deploy --ip-address-type dualstack --tags key=project,value=tolkmeet >/dev/null
for _ in $(seq 1 60); do
	[ "$("${AWS[@]}" lightsail get-instance --instance-name "$NEW" --query instance.state.name --output text)" = running ] && break
	sleep 10
done
# SSH는 지금 이 컴퓨터의 공인 IP에서만 연다(공개 저장소에 운영자 IP를 남기지 않게 실행할 때 채운다).
SSH_CIDR=${SSH_CIDR:-$(curl -fsS https://checkip.amazonaws.com)/32}
"${AWS[@]}" lightsail put-instance-public-ports --instance-name "$NEW" --port-infos "$(sed "s#@SSH_CIDR@#${SSH_CIDR}#" firewall.json)" >/dev/null

echo "moving static IP $IP to $NEW"
"${AWS[@]}" lightsail detach-static-ip --static-ip-name "$IP" >/dev/null 2>&1 || true
"${AWS[@]}" lightsail attach-static-ip --static-ip-name "$IP" --instance-name "$NEW" >/dev/null
sleep 20
ssh-keygen -R 3.39.109.1 >/dev/null 2>&1 || true

echo "restoring the database from S3"
"${SSH[@]}" -o StrictHostKeyChecking=accept-new "$DEPLOY_HOST" '
	set -e
	sudo systemctl stop tolkmeet litestream
	ts=$(date -u +%Y%m%d%H%M%S)
	sudo mkdir -p /var/lib/tolkmeet/before-restore-$ts
	sudo sh -c "mv /var/lib/tolkmeet/tolkmeet.db* /var/lib/tolkmeet/before-restore-$ts/ 2>/dev/null || true"
	# 백업 키(litestream.env)는 root만 읽으므로 root로 되살리고 앱 사용자에게 넘긴다.
	sudo sh -c "set -a; . /etc/tolkmeet/litestream.env; litestream restore -config /etc/litestream.yml /var/lib/tolkmeet/tolkmeet.db"
	sudo chown tolkmeet:tolkmeet /var/lib/tolkmeet/tolkmeet.db*
	sudo systemctl start tolkmeet litestream
	for _ in $(seq 1 30); do curl -fsS -o /dev/null http://127.0.0.1:8080/api/health && break; sleep 1; done
'
curl -fsS --max-time 10 https://meet.tolkmeet.com/api/health >/dev/null && echo "restored: https://meet.tolkmeet.com is up on $NEW"
# 새 서버에도 매일 스냅샷과 상태 점검 알림을 건다(옛 서버 이름으로 걸린 것은 따라오지 않는다).
"${AWS[@]}" lightsail enable-add-on --resource-name "$NEW" --add-on-request 'addOnType=AutoSnapshot,autoSnapshotAddOnRequest={snapshotTimeOfDay=18:00}' >/dev/null || true
"${AWS[@]}" lightsail put-alarm --alarm-name "$NEW-status" --monitored-resource-name "$NEW" --metric-name StatusCheckFailed \
	--comparison-operator GreaterThanOrEqualToThreshold --threshold 1 --evaluation-periods 2 --datapoints-to-alarm 2 \
	--treat-missing-data breaching --contact-protocols Email --notification-triggers ALARM OK --notification-enabled >/dev/null || true
echo "old instance $OLD is kept for inspection; delete it in Lightsail when you no longer need it"
echo "next time, run with SOURCE_INSTANCE=$NEW"
