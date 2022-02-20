//SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.0;

import "../challenge/Challenge.sol";

contract SingleExecutionChallenge is Challenge {
    constructor(
        IOneStepProofEntry osp_,
        IChallengeResultReceiver resultReceiver_,
        uint256 maxInboxMessagesRead_,
        bytes32[2] memory startAndEndHashes,
        uint256 numSteps_,
        address asserter_,
        address challenger_,
        uint256 asserterTimeLeft_,
        uint256 challengerTimeLeft_
    ) {
        osp = osp_;
        resultReceiver = resultReceiver_;
        maxInboxMessages = maxInboxMessagesRead_;
        bytes32[] memory segments = new bytes32[](2);
        segments[0] = startAndEndHashes[0];
        segments[1] = startAndEndHashes[1];
        challengeStateHash = ChallengeLib.hashChallengeState(0, numSteps_, segments);
        asserter = asserter_;
        challenger = challenger_;
        asserterTimeLeft = asserterTimeLeft_;
        challengerTimeLeft = challengerTimeLeft_;
        lastMoveTimestamp = block.timestamp;
        turn = Turn.CHALLENGER;

        emit Bisected(
            challengeStateHash,
            0,
            numSteps_,
            segments
        );
    }
}
