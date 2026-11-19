# Atelier G-code

Application locale en Rust pour comparer des exports Bambu Studio et lancer un plateau tranché sur une Bambu Lab P2S. Le comparateur accepte les fichiers `.gcode` et `.gcode.3mf`. Le lancement utilise l’archive `.gcode.3mf` d’origine et les emplacements de filament sélectionnés dans l’Atelier.

## Démarrer

### Windows 10 ou 11, 64 bits

1. Télécharge **Atelier-Gcode-Setup-x64.exe** depuis la [dernière version publiée](https://github.com/dedetc51/atelier-gcode/releases/latest).
2. Double-clique sur le fichier. L’installation se fait dans ton compte Windows sans droits administrateur.
3. Ouvre **Atelier G-code** depuis le menu **Démarrer**. L’interface s’ouvre dans ton navigateur. Garde la fenêtre de l’Atelier ouverte pendant l’utilisation. Ferme-la pour quitter.

À chaque ouverture depuis le raccourci, l’Atelier vérifie les versions stables du dépôt GitHub en arrière-plan. Il compare la taille et l’empreinte SHA-256 de l’installateur à celles de la publication, puis installe la mise à jour. Le bas de l’interface indique la version et le résultat. La session en cours peut terminer son impression. La nouvelle version s’ouvre au prochain lancement depuis Démarrer. Hors ligne, l’Atelier reste utilisable et réessaie à l’ouverture suivante. [Publications GitHub](https://github.com/dedetc51/atelier-gcode/releases)

Tu peux le désinstaller depuis **Paramètres → Applications → Applications installées** après avoir fermé les fenêtres de l’Atelier. Cette distribution utilise un installateur sans signature Authenticode.

### Linux et compilation depuis les sources

Sous Linux x86-64, utilise l’exécutable fourni avec la distribution Linux, compilé sur Debian 13. Il utilise la bibliothèque système `libzstd.so.1`.

```sh
./atelier-gcode
```

Pour construire la version source, installe Rust stable et les outils de compilation C. La compilation de la bibliothèque TLS embarquée utilise aussi Make et Perl sur Linux et macOS.

```sh
cargo run --locked --release
```

L’Atelier ouvre un navigateur sur `127.0.0.1`. Le terminal indique l’adresse à ouvrir manuellement. `Ctrl+C` ferme le programme.

## Imprimer depuis l’Atelier

1. Dans Bambu Studio, sélectionne le profil **Bambu Lab P2S**, tranche ton plateau et choisis **Exporter le fichier du plateau tranché**. Le fichier obtenu se termine par `.gcode.3mf`.
2. Sur l’écran de la P2S, active **Paramètres → LAN uniquement → Mode développeur**. Relève l’IP, le numéro de série et le code d’accès LAN. **LAN uniquement** désactive Bambu Handy et les fonctions cloud tant que ce mode est actif. [Manuel P2S, §7.3](https://csm.bblcdn.com/hub/9425242c00344b0e801b6179f9a243aa.pdf)
3. Dans l’Atelier, importe l’archive comme référence ou variante et ouvre **Imprimer sur la P2S**. Choisis le plateau, saisis les informations de la P2S et clique sur **Tester la connexion**.
4. Choisis l’emplacement physique correspondant à chaque filament et vérifie la plaque, la buse et l’état du plateau. Coche la confirmation, puis clique sur **Envoyer et lancer l’impression**.

Le bouton de test lit l’état et les emplacements annoncés par la P2S. Le transfert utilise FTPS sur le port 990 et le lancement utilise MQTT sur le port 8883, sur le même réseau local. Le code d’accès reste en mémoire le temps de l’utilisation. La liaison TLS accepte le certificat présenté par la P2S sur ce réseau de confiance.

L’Atelier affiche la confirmation de démarrage quand la P2S la signale. Si le résultat est incertain, consulte l’écran de la P2S avant de renvoyer le plateau.

L’exemple préchargé trace deux carrés synthétiques sur quatre couches. Importe ton archive imprimable pour l’utiliser dans le panneau d’envoi.

## Publier une mise à jour Windows

Augmente la version stable dans `Cargo.toml` (par exemple `0.3.1`), puis lance `cargo check` pour mettre à jour `Cargo.lock`. Une fois ces changements sur `main`, GitHub Actions compile la version x64, teste l’installation, la mise à jour pendant une session et la désinstallation sur Windows. Le workflow crée ensuite le tag `vX.Y.Z` et publie l’installateur dans GitHub Releases. Une version déjà publiée garde son fichier d’origine. Le même workflow peut être relancé depuis l’onglet **Actions**. [Workflow Windows](.github/workflows/windows.yml)
