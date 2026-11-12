#!/bin/sh
# 結合テスト用のダミー session-manager-plugin
if [ "$1" = "--version" ]; then
  echo "1.2.650.0"
  exit 0
fi
echo "The Session Manager plugin was installed successfully. Use the AWS CLI to start a session."
