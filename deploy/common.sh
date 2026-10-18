# 배포 스크립트 공통: 서버 주소와 SSH(1Password의 tolkmeet-deploy 키).
DEPLOY_HOST=${DEPLOY_HOST:-ubuntu@3.39.109.1}
SSH=(ssh -o IdentitiesOnly=yes -i "$HOME/.ssh/tolkmeet-deploy.pub" -o ConnectTimeout=20)
OP=(op --account "${OP_ACCOUNT:-my.1password.com}")
