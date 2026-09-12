use gpui::actions;

actions!(
    git_helper,
    [
        OpenRepository,
        CloneRepository,
        CloseActiveRepository,
        ReopenClosedRepository,
        NextRepository,
        PreviousRepository,
        MoveRepositoryLeft,
        MoveRepositoryRight,
        ToggleFavoriteRepository,
        RefreshRepository,
        ShowHistory,
        ShowChanges,
        CreateCommit,
        GenerateCommitMessage,
    ]
);
