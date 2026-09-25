# herdr-marketplace

La marketplace des plugins Herdr, dans une sidebar : chercher un plugin du
catalogue public, lire son README, l'installer, le passer au commit du
catalogue ou le retirer, sans quitter Herdr ni taper de commande
`herdr plugin`. Le comportement suit celui de la marketplace d'extensions de
VS Code, dans les limites d'un terminal.

## Prérequis

- Herdr **0.9.1**
- Rust **1.89** (édition 2024), pour construire le binaire. Si le Cargo de
  Homebrew passe avant la chaîne installée par rustup, lancez d'abord
  `export PATH="$HOME/.cargo/bin:$PATH"`.
- macOS ou Linux, `git` (Herdr s'en sert pour installer les plugins) et un
  accès réseau (catalogue, README et manifestes viennent de GitHub).

## Lier le checkout local

```sh
git clone https://github.com/massdo/herdr-marketplace
cd herdr-marketplace
sh scripts/build.sh
herdr plugin link "$PWD" --enabled
```

`scripts/build.sh` compile `target/release/herdr-marketplace`, que le
manifeste `herdr-plugin.toml` utilise. Après une mise à jour du checkout,
relancez-le.

## Ouvrir la marketplace

Depuis un pane Herdr :

```sh
herdr plugin action invoke herdr-marketplace.toggle
```

La sidebar s'ouvre à gauche de l'onglet courant, sur environ 32 colonnes ; la
même commande la referme. Pour un raccourci, ajoutez vous-même à votre
`config.toml` :

```toml
[[keys.command]]
key = "prefix+shift+m"
type = "plugin_action"
command = "herdr-marketplace.toggle"
description = "Marketplace"
```

## Parcours complet

1. **Chercher.** Le catalogue se charge à l'ouverture de la sidebar. Tapez :
   la liste se filtre à chaque lettre, sur le nom, l'id, la description,
   owner/repo et les topics. ↑↓, Page préc./suiv., Début et Fin déplacent la
   sélection ; Échap efface la recherche, puis ferme la sidebar. Chaque
   plugin montre son nom, ses étoiles, owner/repo et le début de sa
   description, et les marques « installé », « incompatible » ou « hors
   catalogue ». Les plugins incompatibles non installés sont masqués ; la
   sidebar indique combien.
2. **Lire.** Entrée ouvre la fiche du plugin dans un pane voisin : son README
   rendu, au commit du catalogue. ↑↓, Page préc./suiv., Début et Fin le
   parcourent ; `s` affiche le SHA complet ; Échap ferme la fiche.
3. **Installer.** Dans la fiche, `i` ouvre l'aperçu : source, SHA complet,
   commandes de build et de démarrage, événements, actions et panes. Entrée
   confirme, Échap annule sans rien lancer. L'installation continue si vous
   fermez la fiche ; en la rouvrant, vous retrouvez son résultat, confirmé par
   le registre Herdr.
4. **Changer de commit.** Pour un plugin déjà installé à un autre commit, `i`
   propose de passer au commit du catalogue et affiche les deux SHA.
5. **Retirer.** Dans la fiche d'un plugin installé, `r` puis Entrée.

Une seule installation ou un seul retrait à la fois. Les plugins liés en local,
comme la marketplace elle-même, ne sont ni listés ni retirés.

## Catalogue

La source est l'index public `https://assets.herdr.dev/plugins/index.json`,
rechargé à chaque ouverture de la sidebar. La variable
`HERDR_MARKETPLACE_INDEX_URL` la remplace par une autre URL http(s) ou
`file://`.

## Vérifications

- `sh scripts/check.sh` : format, lint et tests, hors ligne.
- `cargo test -- --ignored` : chargement de l'index réel, par le réseau.
- `sh scripts/e2e.sh` : le parcours complet dans un profil Herdr jetable
  (serveur, configuration et état isolés sous `/tmp`), jamais dans votre
  session quotidienne. Il installe le plugin de test
  `massdo/herdr-marketplace-fixture` depuis GitHub.

## Limites de la V1

Pas de Windows, pas de souris, un seul tri (étoiles), pas de choix de version,
pas de rafraîchissement du catalogue sidebar ouverte. La marketplace
s'utilise liée depuis un checkout local.

Le client du socket Herdr, le dock à gauche et le verrou de lancement
viennent de herdr-npm et de herdr-sidebar (MIT) : voir `NOTICE`.
