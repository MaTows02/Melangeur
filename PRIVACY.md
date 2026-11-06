# Politique de confidentialité de Melangeur

Dernière mise à jour : 6 novembre 2026

**Melangeur n'envoie aucune donnée sur Internet.** Il ne contient ni compte, ni publicité, ni statistiques d'usage.

## Ce que l'extension de navigateur lit

Pour afficher et régler le volume de chaque onglet, l'extension a accès à tous les sites. Elle utilise :

| Donnée | Pourquoi | Où elle va |
|---|---|---|
| Titre et identifiant des onglets qui jouent du son, utilisent le micro ou sont en appel | Les afficher dans Melangeur | Melangeur, sur le même ordinateur |
| État sonore de l'onglet (audible, muet, micro actif, appel en cours) | Afficher les bons indicateurs | Melangeur, sur le même ordinateur |
| Volume choisi pour chaque onglet | Le réappliquer après un rechargement | Mémoire de session du navigateur, effacée à sa fermeture |

L'extension **n'enregistre pas et ne transmet pas le son**, votre voix ou le contenu des pages. Elle ne lit ni l'historique ni les mots de passe, et ne transmet pas les adresses des pages.

Le code qu'elle ajoute aux pages sert à appliquer le volume et à détecter l'usage du micro et les appels.

## Ce que le programme lit

Melangeur lit la liste des sessions audio de Windows et des fenêtres ouvertes. Sans extension, il lit aussi les titres de la barre d'onglets des navigateurs qui ont une session audio, seulement lorsque son panneau est ouvert. Ces titres restent en mémoire le temps de l'affichage et ne sont jamais écrits sur le disque.

## Ce qui reste sur votre ordinateur

- le dossier `%LOCALAPPDATA%\Melangeur` ;
- des entrées de registre pour votre utilisateur, pour relier le navigateur à Melangeur ;
- une entrée de démarrage automatique, si vous l'activez.

La désinstallation retire ces éléments. L'extension se retire depuis la page des extensions du navigateur.

## Contact

https://github.com/MaTows02/Melangeur/issues
