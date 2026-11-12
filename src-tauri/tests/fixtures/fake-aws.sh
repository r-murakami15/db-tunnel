#!/bin/sh
# 結合テスト用のダミー aws コマンド。
# テストごとの一時ディレクトリに `aws` という名前でシンボリックリンクを置いて使う。
# 動作は同じディレクトリの scenario.env（シェル変数）で切り替える:
#   STS=ok|login|fail      get-caller-identity（login: logged-in ファイルがあれば成功）
#   LOGIN=ok|fail|slow     aws login（slow: 1秒待ってから成功）
#   BASTIONS=0|1|2|denied  ec2 describe-instances の件数
#   RDS=ok|none|nosecret   rds describe-db-instances
#   SECRET=ok|bad|denied   secretsmanager get-secret-value
#   SSM=ok|exit|timeout|denied  start-session（ok: 接続して待ち続ける）
DIR=$(cd "$(dirname "$0")" && pwd)
STS=ok LOGIN=ok BASTIONS=1 RDS=ok SECRET=ok SSM=ok
[ -f "$DIR/scenario.env" ] && . "$DIR/scenario.env"
echo "$*" >> "$DIR/calls.log"

if [ "$1" = "--version" ]; then
  echo "aws-cli/2.31.3 Python/3.13.7 Linux/6.0 source/x86_64"
  exit 0
fi

case "$1 $2" in
  "sts get-caller-identity")
    if [ "$STS" = ok ] || { [ "$STS" = login ] && [ -f "$DIR/logged-in" ]; }; then
      echo '{'
      echo '    "UserId": "AROAEXAMPLE:test",'
      echo '    "Account": "123456789012",'
      echo '    "Arn": "arn:aws:sts::123456789012:assumed-role/Dev/test"'
      echo '}'
      exit 0
    fi
    echo "Error when retrieving token from sso: Token has expired and refresh failed" >&2
    exit 255
    ;;
  login*)
    echo "Attempting to open your default browser."
    case "$LOGIN" in
      ok) touch "$DIR/logged-in"; echo "Successfully logged in."; exit 0 ;;
      slow) sleep 1; touch "$DIR/logged-in"; echo "Successfully logged in."; exit 0 ;;
      *) echo "Login failed" >&2; exit 1 ;;
    esac
    ;;
  "ec2 describe-instances")
    case "$BASTIONS" in
      0) echo '[]' ;;
      1) echo '[{"id": "i-0123456789abcdef0", "name": "acme-portal-dev-bastion"}]' ;;
      2) echo '[{"id": "i-0aaa", "name": "acme-portal-dev-bastion"},'
         echo ' {"id": "i-0bbb", "name": "acme-batch-dev-bastion"}]' ;;
      denied)
        echo "" >&2
        echo "An error occurred (UnauthorizedOperation) when calling the DescribeInstances operation: You are not authorized to perform this operation." >&2
        exit 254 ;;
    esac
    exit 0
    ;;
  "rds describe-db-instances")
    case "$RDS" in
      none) echo '{"DBInstances": [{"DBInstanceIdentifier": "other-dev-mysql"}]}' ;;
      nosecret) echo '{"DBInstances": [{"DBInstanceIdentifier": "acme-portal-dev-mysql", "Endpoint": {"Address": "acme-portal-dev-mysql.abc.ap-northeast-1.rds.amazonaws.com"}}]}' ;;
      *)
        cat <<JSON
{
    "DBInstances": [
        {
            "DBInstanceIdentifier": "acme-portal-dev-mysql",
            "Endpoint": {"Address": "acme-portal-dev-mysql.abc.ap-northeast-1.rds.amazonaws.com", "Port": 3306},
            "MasterUserSecret": {"SecretArn": "arn:aws:secretsmanager:ap-northeast-1:123456789012:secret:rds!db-1234"}
        },
        {
            "DBInstanceIdentifier": "acme-portal-stg-mysql",
            "Endpoint": {"Address": "acme-portal-stg-mysql.abc.ap-northeast-1.rds.amazonaws.com", "Port": 3306}
        },
        {
            "DBInstanceIdentifier": "other-dev-mysql"
        }
    ]
}
JSON
        ;;
    esac
    exit 0
    ;;
  "secretsmanager get-secret-value")
    case "$SECRET" in
      ok) echo '{"username":"admin","password":"s3cret!"}' ;;
      bad) echo 'not-json s3cret!' ;;
      denied)
        echo "An error occurred (AccessDeniedException) when calling the GetSecretValue operation: denied" >&2
        exit 254 ;;
    esac
    exit 0
    ;;
  "ssm start-session")
    echo $$ > "$DIR/session.pid"
    PORT=0
    for a in "$@"; do
      case "$a" in
        *localPortNumber=*) PORT=${a##*localPortNumber=} ;;
      esac
    done
    if [ "$SSM" = denied ]; then
      echo "An error occurred (AccessDeniedException) when calling the StartSession operation: denied" >&2
      exit 254
    fi
    echo ""
    echo "Starting session with SessionId: test-0123"
    echo "Port $PORT opened for sessionId test-0123."
    echo "Waiting for connections..."
    case "$SSM" in
      exit) sleep 1; echo "Connection lost" >&2; exit 3 ;;
      timeout) sleep 1; echo "SessionId: test-0123 : Your session timed out due to inactivity and has been terminated."; exit 0 ;;
    esac
    # session-manager-plugin の代わりに孫プロセスを残す（停止時にグループごと終了されるか確認する）
    sleep 600 &
    echo $! > "$DIR/plugin.pid"
    wait
    ;;
  *)
    echo "fake-aws: unknown command: $*" >&2
    exit 2
    ;;
esac
