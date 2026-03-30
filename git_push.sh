#!/bin/bash
# push_all.sh - wypycha aktualny stan repozytorium na GitHub i Codeberg

REPO_PATH="$HOME/Dokumenty/CosinusOS_"
GITHUB_REMOTE="git@github.com:Ctrl-F0rg3/CosinusOS.git"
CODEBERG_REMOTE="git@codeberg.org:Ctrl-F0rg3/CosinusOS.git"
LOGFILE="$REPO_PATH/push_log.txt"

cd "$REPO_PATH" || { echo "Nie można wejść do repozytorium"; exit 1; }

echo "=== $(date) ===" >> "$LOGFILE"
echo "Push na GitHub..." | tee -a "$LOGFILE"
git push --mirror "$GITHUB_REMOTE" 2>&1 | tee -a "$LOGFILE"

echo "Push na Codeberg..." | tee -a "$LOGFILE"
git push --mirror "$CODEBERG_REMOTE" 2>&1 | tee -a "$LOGFILE"

echo "Push zakończony: $(date)" | tee -a "$LOGFILE"