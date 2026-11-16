#!/bin/sh
# Construit la documentation et la publie sur lion.mbny.fr (serveur mbny.fr, dossier ~/serveur/sites/lion).
set -e
cd "$(dirname "$0")"
python3 construire.py
rsync -a --delete _site/ mbny@100.71.191.53:serveur/sites/lion/
echo "Publié : https://lion.mbny.fr"
